#![no_std]
#![no_main]

mod ble;
mod bootloader;
mod input;
mod leds;
mod pins;

use esp_backtrace as _;

use embassy_executor::Spawner;
use esp_hal::rmt::TxChannelCreator;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use openmicro_effects::Rgb;
use openmicro_proto::{Battery, InputEvent, LedFrame};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

static LED_FRAME_CHANNEL: Channel<CriticalSectionRawMutex, LedFrame, 2> = Channel::new();

static INPUT_EVENT_CHANNEL: Channel<CriticalSectionRawMutex, InputEvent, 8> = Channel::new();

static BATTERY_CHANNEL: Channel<CriticalSectionRawMutex, Battery, 1> = Channel::new();

const HEAP_SIZE: usize = 64 * 1024;

struct RmtWriter<const N: usize> {
    channel: Option<esp_hal::rmt::Channel<'static, esp_hal::Blocking, esp_hal::rmt::Tx>>,
    codes: [esp_hal::rmt::PulseCode; N],
}

const fn ticks(ns: u32) -> u16 {
    (ns * 8 / 100) as u16
}

const T0H: u16 = ticks(400);
const T0L: u16 = ticks(850);
const T1H: u16 = ticks(800);
const T1L: u16 = ticks(450);

impl<const N: usize> RmtWriter<N> {
    fn new(channel: esp_hal::rmt::Channel<'static, esp_hal::Blocking, esp_hal::rmt::Tx>) -> Self {
        Self {
            channel: Some(channel),
            codes: [esp_hal::rmt::PulseCode(0); N],
        }
    }
}

impl<const N: usize> leds::PixelOut for RmtWriter<N> {
    fn write_pixels(&mut self, pixels: &[Rgb]) -> Result<(), ()> {
        use esp_hal::gpio::Level;
        use esp_hal::rmt::PulseCode;

        let needed = pixels.len() * 24 + 1;
        if needed > N {
            return Err(());
        }
        let mut at = 0;
        for px in pixels {
            let px = openmicro_effects::gamma(*px);
            for byte in [px.g, px.r, px.b] {
                for bit in (0..8).rev() {
                    self.codes[at] = if (byte >> bit) & 1 == 1 {
                        PulseCode::new(Level::High, T1H, Level::Low, T1L)
                    } else {
                        PulseCode::new(Level::High, T0H, Level::Low, T0L)
                    };
                    at += 1;
                }
            }
        }
        self.codes[at] = PulseCode(0);

        let channel = self.channel.take().ok_or(())?;
        match channel.transmit(&self.codes[..=at]) {
            Ok(tx) => match tx.wait() {
                Ok(ch) => {
                    self.channel = Some(ch);
                    Ok(())
                }
                Err((_, ch)) => {
                    self.channel = Some(ch);
                    Err(())
                }
            },
            Err((_, ch)) => {
                self.channel = Some(ch);
                Err(())
            }
        }
    }
}

const PER_KEY_CODES: usize = pins::PER_KEY_LED_COUNT * 24 + 1;
const UNDERGLOW_CODES: usize = pins::UNDERGLOW_LED_COUNT * 24 + 1;

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger(log::LevelFilter::Info);
    esp_println::println!("openmicro-fw boot");

    bootloader::clear_force_download();
    log::info!("openmicro-fw {} starting", env!("CARGO_PKG_VERSION"));

    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));

    esp_alloc::heap_allocator!(size: HEAP_SIZE);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let ble_controller =
        esp_radio::ble::controller::BleConnector::new(peripherals.BT, Default::default())
            .expect("BLE controller init");

    let rmt = esp_hal::rmt::Rmt::new(peripherals.RMT, esp_hal::time::Rate::from_mhz(80))
        .expect("rmt init");
    let rmt_cfg = esp_hal::rmt::TxChannelConfig::default()
        .with_clk_divider(1)
        .with_idle_output_level(esp_hal::gpio::Level::Low)
        .with_idle_output(true)
        .with_carrier_modulation(false);
    let per_key_leds: RmtWriter<PER_KEY_CODES> = RmtWriter::new(
        rmt.channel0
            .configure_tx(&rmt_cfg)
            .expect("rmt ch0")
            .with_pin(peripherals.GPIO7),
    );
    let underglow_leds: RmtWriter<UNDERGLOW_CODES> = RmtWriter::new(
        rmt.channel1
            .configure_tx(&rmt_cfg)
            .expect("rmt ch1")
            .with_pin(peripherals.GPIO6),
    );

    spawner.spawn(ble_task(ble_controller).expect("ble_task token"));
    spawner.spawn(
        led_render_task(
            per_key_leds,
            underglow_leds,
            input_pin(peripherals.GPIO42, esp_hal::gpio::Pull::Up),
        )
        .expect("led_render_task token"),
    );
    let _top_board_power = [
        esp_hal::gpio::Output::new(
            peripherals.GPIO36,
            level_of(pins::TOP_BOARD_POWER[0].1),
            esp_hal::gpio::OutputConfig::default(),
        ),
        esp_hal::gpio::Output::new(
            peripherals.GPIO37,
            level_of(pins::TOP_BOARD_POWER[1].1),
            esp_hal::gpio::OutputConfig::default(),
        ),
        esp_hal::gpio::Output::new(
            peripherals.GPIO38,
            level_of(pins::TOP_BOARD_POWER[2].1),
            esp_hal::gpio::OutputConfig::default(),
        ),
    ];
    core::mem::forget(_top_board_power);
    esp_println::println!("top board powered (held)");

    let joystick_adc = {
        let mut cfg = esp_hal::analog::adc::AdcConfig::new();
        let x = cfg.enable_pin(peripherals.GPIO9, esp_hal::analog::adc::Attenuation::_11dB);
        let y = cfg.enable_pin(peripherals.GPIO10, esp_hal::analog::adc::Attenuation::_11dB);
        (esp_hal::analog::adc::Adc::new(peripherals.ADC1, cfg), x, y)
    };

    spawner.spawn(
        input_task(InputPins {
            adc: joystick_adc.0,
            joy_x: joystick_adc.1,
            joy_y: joystick_adc.2,
            drive: [
                esp_hal::gpio::Output::new(
                    peripherals.GPIO46,
                    esp_hal::gpio::Level::Low,
                    esp_hal::gpio::OutputConfig::default(),
                ),
                esp_hal::gpio::Output::new(
                    peripherals.GPIO17,
                    esp_hal::gpio::Level::Low,
                    esp_hal::gpio::OutputConfig::default(),
                ),
                esp_hal::gpio::Output::new(
                    peripherals.GPIO40,
                    esp_hal::gpio::Level::Low,
                    esp_hal::gpio::OutputConfig::default(),
                ),
                esp_hal::gpio::Output::new(
                    peripherals.GPIO47,
                    esp_hal::gpio::Level::Low,
                    esp_hal::gpio::OutputConfig::default(),
                ),
            ],
            sense: [
                input_pin(peripherals.GPIO13, esp_hal::gpio::Pull::Down),
                input_pin(peripherals.GPIO5, esp_hal::gpio::Pull::Down),
                input_pin(peripherals.GPIO21, esp_hal::gpio::Pull::Down),
                input_pin(peripherals.GPIO1, esp_hal::gpio::Pull::Down),
            ],
            encoder_a: input_pin(peripherals.GPIO12, esp_hal::gpio::Pull::Up),
            encoder_b: input_pin(peripherals.GPIO11, esp_hal::gpio::Pull::Up),
            encoder_sw: input_pin(peripherals.GPIO4, esp_hal::gpio::Pull::Up),
        })
        .expect("input_task token"),
    );
    let (serial_rx, serial_tx) =
        esp_hal::usb_serial_jtag::UsbSerialJtag::new(peripherals.USB_DEVICE).split();
    core::mem::forget(serial_tx);
    spawner.spawn(serial_command_task(serial_rx).expect("serial_command_task token"));
    spawner.spawn(serial_input_task().expect("serial_input_task token"));

    spawner.spawn(battery_task().expect("battery_task token"));
    spawner.spawn(
        power_task(input_pin(peripherals.GPIO2, esp_hal::gpio::Pull::Up))
            .expect("power_task token"),
    );
}

#[embassy_executor::task]
async fn ble_task(controller: esp_radio::ble::controller::BleConnector<'static>) {
    let _ = controller;
    loop {
        embassy_time::Timer::after_secs(3600).await;
    }
}

#[embassy_executor::task]
async fn power_task(button: esp_hal::gpio::Input<'static>) {
    use openmicro_effects::power::{PowerAction, PowerButton};

    let mut gesture = PowerButton::new();
    let start = embassy_time::Instant::now();
    loop {
        let pressed = button.is_low();
        let now_ms = start.elapsed().as_millis() as u32;

        match gesture.update(pressed, now_ms) {
            PowerAction::EnterBootloader => bootloader::reboot_to_bootloader(),
            PowerAction::PowerOff => {
            }
            PowerAction::Wake => touch_activity(now_ms),
            PowerAction::None => {}
        }
        embassy_time::Timer::after_millis(10).await;
    }
}

#[embassy_executor::task]
async fn led_render_task(
    per_key: RmtWriter<PER_KEY_CODES>,
    underglow: RmtWriter<UNDERGLOW_CODES>,
    usb_detect: esp_hal::gpio::Input<'static>,
) {
    use openmicro_effects::status::{self, Link};

    log::info!(
        "leds: per-key {} on GPIO{}, underglow {} on GPIO{}",
        pins::PER_KEY_LED_COUNT,
        pins::PER_KEY_LED_GPIO,
        pins::UNDERGLOW_LED_COUNT,
        pins::UNDERGLOW_LED_GPIO
    );
    let mut keys = leds::KeyChain::new(per_key);
    let mut glow = leds::GlowRing::new(underglow);
    let mut ticker =
        embassy_time::Ticker::every(embassy_time::Duration::from_millis(leds::RENDER_PERIOD_MS));
    let start = embassy_time::Instant::now();
    let mut last_beat_ms = 0u32;
    let mut asleep = false;
    let mut last_link: Option<Link> = None;
    let mut last_demo_index: Option<usize> = None;
    let mut held = LedFrame::BLANK;

    loop {
        let t_ms = start.elapsed().as_millis() as u32;

        if display_mode() == MODE_PROBE {
            keys.set_chain_indices(&[
                (PROBE_FIRST, Rgb { r: 255, g: 0, b: 0 }),
                (PROBE_MIDDLE, Rgb { r: 0, g: 255, b: 0 }),
                (PROBE_LAST, Rgb { r: 0, g: 0, b: 255 }),
            ]);
            glow.set_chain_index(0, Rgb { r: 255, g: 255, b: 255 });
            if t_ms.wrapping_sub(last_beat_ms) >= 3000 {
                last_beat_ms = t_ms;
                esp_println::println!(
                    "probe: chain {} red, {} green, {} blue",
                    PROBE_FIRST,
                    PROBE_MIDDLE,
                    PROBE_LAST
                );
            }
            ticker.next().await;
            continue;
        }

        if display_mode() == MODE_IDENTIFY {
            let step = ((t_ms / IDENTIFY_DWELL_MS) % (pins::PER_KEY_LED_COUNT as u32)) as usize;
            keys.set_chain_index(step, Rgb { r: 255, g: 255, b: 255 });
            let ring_step =
                ((t_ms / IDENTIFY_DWELL_MS) % (pins::UNDERGLOW_LED_COUNT as u32)) as usize;
            glow.set_chain_index(ring_step, Rgb { r: 255, g: 120, b: 0 });
            if t_ms.wrapping_sub(last_beat_ms) >= IDENTIFY_DWELL_MS {
                last_beat_ms = t_ms;
                esp_println::println!("identify: key chain index {} / ring {}", step, ring_step);
            }
            ticker.next().await;
            continue;
        }

        if display_mode() == MODE_DEMO {
            use openmicro_effects::demo;
            let (index, step) = demo::scene_at(t_ms);
            if last_demo_index != Some(index) {
                last_demo_index = Some(index);
                esp_println::println!("demo {}/{}: {}", index + 1, demo::SCENE_COUNT, step.label);
            }
            match step.scene {
                demo::Scene::Local(link) => {
                    keys.blank();
                    glow.render_link(link, demo::DEMO_BRIGHTNESS, t_ms);
                }
                demo::Scene::Host(frame) => {
                    keys.render(&frame, t_ms);
                    glow.render(&frame.glow, t_ms);
                }
            }
            ticker.next().await;
            continue;
        }

        while let Ok(frame) = LED_FRAME_CHANNEL.try_receive() {
            held = frame;
            LAST_FRAME_MS.store(t_ms, core::sync::atomic::Ordering::Relaxed);
            if frame != LedFrame::BLANK {
                touch_activity(t_ms);
            }
        }

        let idle_for = t_ms.wrapping_sub(LAST_ACTIVITY_MS.load(core::sync::atomic::Ordering::Relaxed));
        let since_frame =
            t_ms.wrapping_sub(LAST_FRAME_MS.load(core::sync::atomic::Ordering::Relaxed));
        let host_attached = usb_detect.is_low();
        let link = status::link_state(host_attached, since_frame);

        if openmicro_effects::startup::is_running(t_ms) {
            keys.render_startup(t_ms);
            glow.render_startup(t_ms);
            if t_ms < 40 {
                esp_println::println!(
                    "led: first frame pushed ({} + {} bytes)",
                    leds::PER_KEY_BUF_LEN,
                    leds::UNDERGLOW_BUF_LEN
                );
            }
        } else if idle_for >= LED_SLEEP_MS {
            if !asleep {
                asleep = true;
                esp_println::println!("leds: sleeping after {} ms idle", LED_SLEEP_MS);
            }
            keys.blank();
            glow.blank();
        } else {
            if asleep {
                asleep = false;
                esp_println::println!("leds: awake");
            }
            if last_link != Some(link) {
                last_link = Some(link);
                esp_println::println!("link: {:?} (usb={})", link, host_attached);
            }
            match link {
                Link::Live => {
                    keys.render(&held, t_ms);
                    glow.render(&held.glow, t_ms);
                }
                Link::NoDaemon | Link::Offline => {
                    keys.blank();
                    glow.render_link(link, LOCAL_BRIGHTNESS, t_ms);
                }
            }
        }

        if t_ms.wrapping_sub(last_beat_ms) >= 2000 {
            last_beat_ms = t_ms;
            esp_println::println!("alive t={}ms link={:?}", t_ms, link);
        }

        ticker.next().await;
    }
}

const LED_SLEEP_MS: u32 = 5 * 60 * 1000;

static LAST_ACTIVITY_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

static LAST_FRAME_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn touch_activity(now_ms: u32) {
    LAST_ACTIVITY_MS.store(now_ms, core::sync::atomic::Ordering::Relaxed);
}

const LOCAL_BRIGHTNESS: u8 = 255;

static DISPLAY_MODE: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(INITIAL_DISPLAY_MODE);

pub const MODE_NORMAL: u8 = 0;
pub const MODE_DEMO: u8 = 1;
pub const MODE_IDENTIFY: u8 = 2;
pub const MODE_PROBE: u8 = 3;

const INITIAL_DISPLAY_MODE: u8 = if option_env!("OPENMICRO_DEMO").is_some() {
    MODE_DEMO
} else if option_env!("OPENMICRO_IDENTIFY").is_some() {
    MODE_IDENTIFY
} else {
    MODE_NORMAL
};

const RX_CHUNK: usize = 64;

const TRACE_CABLE_BYTES: bool = option_env!("OPENMICRO_TRACE_RX").is_some();

pub const IDENTITY: &str = "openmicro-fw";

fn display_mode() -> u8 {
    DISPLAY_MODE.load(core::sync::atomic::Ordering::Relaxed)
}

pub const COMMAND_PREFIX: u8 = b'!';

fn handle_serial_command(byte: u8, armed: &mut bool) {
    if !*armed {
        *armed = byte == COMMAND_PREFIX;
        return;
    }
    *armed = false;
    let mode = match byte {
        b'n' | b'N' => MODE_NORMAL,
        b'd' | b'D' => MODE_DEMO,
        b'i' | b'I' => MODE_IDENTIFY,
        b'p' | b'P' => MODE_PROBE,
        b'?' => {
            esp_println::println!("{} {}", IDENTITY, env!("CARGO_PKG_VERSION"));
            return;
        }
        other => {
            esp_println::println!(
                "mode: unknown command {:?} (want !n, !d, !i or !?)",
                other as char
            );
            return;
        }
    };
    DISPLAY_MODE.store(mode, core::sync::atomic::Ordering::Relaxed);
    let name = match mode {
        MODE_DEMO => "demo",
        MODE_IDENTIFY => "identify",
        MODE_PROBE => "probe",
        _ => "normal",
    };
    esp_println::println!("mode: {}", name);
}

#[embassy_executor::task]
async fn serial_command_task(
    mut rx: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, esp_hal::Blocking>,
) {
    use openmicro_proto::wire;

    esp_println::println!("cable: ready ('!?' to identify, '!d' demo, '!n' normal)");
    let mut reader = wire::Reader::new();
    let mut armed = false;

    loop {
        let mut chunk = [0u8; RX_CHUNK];
        let got = rx.drain_rx_fifo(&mut chunk);
        for &byte in &chunk[..got] {
            if TRACE_CABLE_BYTES {
                esp_println::println!("rx {:02x}", byte);
            }
            match reader.push(byte) {
                wire::Feed::Frame => match LedFrame::decode(reader.frame()) {
                    Ok(frame) => {
                        if LED_FRAME_CHANNEL.try_send(frame).is_err() {
                            let _ = LED_FRAME_CHANNEL.try_receive();
                            let _ = LED_FRAME_CHANNEL.try_send(frame);
                        }
                    }
                    Err(_) => esp_println::println!("cable: undecodable frame"),
                },
                wire::Feed::Bad => esp_println::println!("cable: bad checksum"),
                wire::Feed::None => {
                    if !reader.in_frame() && byte.is_ascii() {
                        handle_serial_command(byte, &mut armed);
                    }
                }
            }
        }
        embassy_time::Timer::after_millis(5).await;
    }
}

#[embassy_executor::task]
async fn serial_input_task() {
    use openmicro_proto::wire;

    loop {
        let event = INPUT_EVENT_CHANNEL.receive().await;
        let Ok(payload) = event.encode() else { continue };
        let mut framed = [0u8; wire::MAX_PAYLOAD + 3];
        if let Some(n) = wire::encode(&payload, &mut framed) {
            esp_println::Printer::write_bytes(&framed[..n]);
        }
    }
}

const IDENTIFY_DWELL_MS: u32 = 1500;

const PROBE_FIRST: usize = 0;
const PROBE_MIDDLE: usize = pins::PER_KEY_LED_COUNT / 2;
const PROBE_LAST: usize = pins::PER_KEY_LED_COUNT - 1;

fn level_of(high: bool) -> esp_hal::gpio::Level {
    if high {
        esp_hal::gpio::Level::High
    } else {
        esp_hal::gpio::Level::Low
    }
}

fn input_pin<'d>(
    pin: impl esp_hal::gpio::InputPin + 'd,
    pull: esp_hal::gpio::Pull,
) -> esp_hal::gpio::Input<'d> {
    esp_hal::gpio::Input::new(pin, esp_hal::gpio::InputConfig::default().with_pull(pull))
}

struct InputPins {
    adc: esp_hal::analog::adc::Adc<'static, esp_hal::peripherals::ADC1<'static>, esp_hal::Blocking>,
    joy_x: esp_hal::analog::adc::AdcPin<
        esp_hal::peripherals::GPIO9<'static>,
        esp_hal::peripherals::ADC1<'static>,
    >,
    joy_y: esp_hal::analog::adc::AdcPin<
        esp_hal::peripherals::GPIO10<'static>,
        esp_hal::peripherals::ADC1<'static>,
    >,
    drive: [esp_hal::gpio::Output<'static>; pins::MATRIX_DRIVE_COUNT],
    sense: [esp_hal::gpio::Input<'static>; pins::MATRIX_SENSE_COUNT],
    encoder_a: esp_hal::gpio::Input<'static>,
    encoder_b: esp_hal::gpio::Input<'static>,
    encoder_sw: esp_hal::gpio::Input<'static>,
}

#[embassy_executor::task]
async fn input_task(mut io: InputPins) {
    let mut matrix = input::MatrixState::new();
    let start = embassy_time::Instant::now();
    let mut prev_ab = ab_state(&io.encoder_a, &io.encoder_b);
    let mut sw_was_down = io.encoder_sw.is_low();
    let mut last_joystick_ms = 0u32;

    loop {
        let now_ms = start.elapsed().as_millis() as u32;

        let mut raw = [[false; pins::MATRIX_SENSE_COUNT]; pins::MATRIX_DRIVE_COUNT];
        for (d, drive) in io.drive.iter_mut().enumerate() {
            drive.set_high();
            embassy_time::Timer::after_micros(20).await;
            for (s, sense) in io.sense.iter().enumerate() {
                raw[d][s] = sense.is_high() == pins::MATRIX_ACTIVE_HIGH;
            }
            drive.set_low();
        }
        matrix.debounce(&raw, now_ms, |event| {
            touch_activity(now_ms);
            log::info!("input: {:?}", event);
            let _ = INPUT_EVENT_CHANNEL.try_send(event);
        });

        let ab = ab_state(&io.encoder_a, &io.encoder_b);
        if ab != prev_ab {
            let step = input::encoder_step(prev_ab, ab);
            prev_ab = ab;
            if step != 0 {
                touch_activity(now_ms);
                let _ = INPUT_EVENT_CHANNEL.try_send(InputEvent::Encoder { delta: step });
            }
        }

        let sw_down = io.encoder_sw.is_low();
        if sw_down != sw_was_down {
            sw_was_down = sw_down;
            touch_activity(now_ms);
            let _ = INPUT_EVENT_CHANNEL.try_send(InputEvent::Key {
                id: pins::ENCODER_PRESS_KEY_ID,
                pressed: sw_down,
            });
        }

        if now_ms.wrapping_sub(last_joystick_ms) >= JOYSTICK_PERIOD_MS {
            last_joystick_ms = now_ms;
            let x_raw = io.adc.read_blocking(&mut io.joy_x);
            let y_raw = io.adc.read_blocking(&mut io.joy_y);
            let x = if pins::JOYSTICK_X_INVERTED {
                ADC_MAX.saturating_sub(x_raw)
            } else {
                x_raw
            };
            if let Some(event) = input::joystick_to_sector(x, y_raw, ADC_CENTRE, JOYSTICK_DEADZONE)
            {
                touch_activity(now_ms);
                let _ = INPUT_EVENT_CHANNEL.try_send(event);
            }
        }

        embassy_time::Timer::after_millis(2).await;
    }
}

const ADC_MAX: u16 = 4095;
const ADC_CENTRE: u16 = 2048;
const JOYSTICK_DEADZONE: u16 = 700;
const JOYSTICK_PERIOD_MS: u32 = 40;

fn ab_state(a: &esp_hal::gpio::Input<'static>, b: &esp_hal::gpio::Input<'static>) -> u8 {
    ((a.is_high() as u8) << 1) | (b.is_high() as u8)
}

#[embassy_executor::task]
async fn battery_task() {
    loop {
        embassy_time::Timer::after_secs(30).await;
    }
}

static _RESOURCES_CELL: StaticCell<()> = StaticCell::new();
