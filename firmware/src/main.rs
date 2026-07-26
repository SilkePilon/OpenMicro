//! OpenMicro ESP32-S3 firmware — embedded skeleton.
//!
//! # Status: COMPILES ON CI, NEVER RUN ON HARDWARE
//!
//! This crate builds for `xtensa-esp32s3-none-elf` on CI
//! (`.github/workflows/firmware.yml`), against the version set pinned in
//! `Cargo.toml` (esp-hal 1.1.1 + esp-rtos 0.3.0 + esp-radio 0.18.0 +
//! trouble-host 0.6.0 — the same set as the upstream `esp-hal-v1.1.1`
//! `bas_peripheral` BLE example). Compiling is all it is proven to do: it has
//! never been flashed to or run on a device.
//!
//! The GPIO map in `pins.rs` is real, recovered from Work Louder's own
//! published firmware (`docs/hardware/creator-micro-2-pinout-findings.md`).
//! What is still missing is the HAL plumbing in the task bodies below.
//!
//! # Keeping the vendor's behaviour
//!
//! Two things about the stock firmware are worth preserving, and are:
//! - the **power button** — short press wakes, ~2 s turns off (see
//!   `openmicro_effects::power`, which also adds a long hold as a
//!   bootloader escape hatch);
//! - **entering and leaving the ROM bootloader on command** (`bootloader.rs`).
//!   Unlike the vendor's, this firmware exposes USB-Serial-JTAG, so the host
//!   can also reset it into download mode with esptool directly — that path
//!   keeps working even if this firmware is wedged.
//!
//! # Task layout
//!
//! Four embassy tasks, wired together with `embassy-sync` channels:
//! - `ble_task`: TrouBLE GATT server (`ble.rs`) — receives `LedFrame`
//!   writes from the host, forwards decoded frames to `led_render_task`;
//!   drains `InputEvent`s produced by `input_task` and notifies them.
//! - `led_render_task`: every `leds::RENDER_PERIOD_MS`, resolves the latest
//!   `LedFrame` (via `openmicro_effects::resolve`, the host-tested effect
//!   core) and pushes it out over RMT (`leds.rs`).
//! - `input_task`: scans the key matrix / encoder / joystick (`input.rs`)
//!   and pushes `InputEvent`s to the BLE task.
//! - `battery_task`: reads the MAX77972 fuel gauge over I2C and updates the
//!   Battery Service characteristic via the BLE task.
//! - `power_task`: debounces the rear power button and acts on the gesture.

#![no_std]
#![no_main]

mod ble;
mod bootloader;
mod input;
mod leds;
mod pins;

// Registers esp-backtrace's `#[panic_handler]` (and, on panic, prints a
// backtrace via esp-println) — required because `#![no_std]` binaries must
// supply their own panic handler and this crate never calls into
// `esp_backtrace` directly otherwise.
use esp_backtrace as _;

use embassy_executor::Spawner;
use esp_hal::rmt::TxChannelCreator;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use openmicro_effects::Rgb;
use openmicro_proto::{Battery, InputEvent, LedFrame};
use static_cell::StaticCell;

// esp-hal 1.1's boot flow requires the application to embed an ESP-IDF app
// descriptor for the 2nd-stage bootloader / espflash to accept the image.
esp_bootloader_esp_idf::esp_app_desc!();

/// Host -> firmware: decoded LED frames from the BLE write characteristic.
/// Depth 2 so a fresh write can supersede one not yet rendered without
/// blocking the BLE task.
static LED_FRAME_CHANNEL: Channel<CriticalSectionRawMutex, LedFrame, 2> = Channel::new();

/// Firmware -> host: input events awaiting a BLE notification. Depth 8
/// gives the BLE task room to drain a burst of key presses / encoder ticks
/// without the input task blocking.
static INPUT_EVENT_CHANNEL: Channel<CriticalSectionRawMutex, InputEvent, 8> = Channel::new();

/// Firmware -> host: latest battery reading, for the Battery Service.
static BATTERY_CHANNEL: Channel<CriticalSectionRawMutex, Battery, 1> = Channel::new();

/// Heap for `alloc` users (`openmicro-proto`'s postcard encode/decode uses
/// `alloc::vec::Vec`; TrouBLE/esp-radio may also need heap for connection
/// bookkeeping depending on configuration). Size is a placeholder — tune
/// once real memory pressure is measured on hardware.
const HEAP_SIZE: usize = 64 * 1024;

/// Drives one WS2812 chain from an RMT channel.
///
/// RMT rather than SPI. The SPI path was matched to ESP-IDF's `led_strip`
/// backend exactly — same 2.5 MHz clock, same three-SPI-bits-per-LED-bit
/// `100`/`110` encoding, same GRB order, same DMA — and it accepted every
/// transfer without error while lighting nothing, on a board where the vendor
/// firmware lights the same chains happily. RMT is the peripheral built for
/// this: the timing is stated in nanoseconds rather than smuggled through a
/// clock divider and a bit pattern.
struct RmtWriter<const N: usize> {
    channel: Option<esp_hal::rmt::Channel<'static, esp_hal::Blocking, esp_hal::rmt::Tx>>,
    codes: [esp_hal::rmt::PulseCode; N],
}

/// RMT ticks for a duration in nanoseconds, at 80 MHz with divider 1 (12.5 ns
/// per tick). Written as a divide so the constants below stay in nanoseconds.
const fn ticks(ns: u32) -> u16 {
    (ns * 8 / 100) as u16
}

// WS2812B datasheet intervals.
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
            // Gamma-encode here and nowhere else: this is the single output
            // boundary, so every animation gets it exactly once. Without it a
            // linear crossfade between adjacent LEDs looks like a snap, which
            // reads as a choppy comet however smooth the arithmetic is.
            let px = openmicro_effects::gamma(*px);
            // Wire order is green, red, blue.
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
        // An all-zero code ends the sequence; the channel is configured to idle
        // low, so the line then holds down and that is the inter-frame latch.
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

/// Per-key chain pulse-code buffer length.
const PER_KEY_CODES: usize = pins::PER_KEY_LED_COUNT * 24 + 1;
/// Underglow chain pulse-code buffer length.
const UNDERGLOW_CODES: usize = pins::UNDERGLOW_LED_COUNT * 24 + 1;

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    // Fixed level rather than `init_logger_from_env`: that reads ESP_LOG at
    // compile time, and a missing or mis-plumbed value silently filters every
    // message, which is indistinguishable from the firmware being dead.
    esp_println::logger::init_logger(log::LevelFilter::Info);
    esp_println::println!("openmicro-fw boot");

    // Clear the force-download-boot bit first thing. It survives a reset — and
    // on the Pro, whose RTC domain is battery-backed, it survives losing USB
    // power too — so a device that was put into download mode once would
    // otherwise keep going back there on every boot.
    bootloader::clear_force_download();
    log::info!("openmicro-fw {} starting", env!("CARGO_PKG_VERSION"));

    // esp-hal 1.x peripheral init. `esp_hal::init` hands back the
    // peripheral singletons used to construct every driver below.
    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));

    // Global allocator. `esp_alloc::heap_allocator!` is a safe macro that
    // sets up a `#[global_allocator]` backed by a static buffer of
    // `HEAP_SIZE` bytes; no raw pointer/unsafe code needed at this call
    // site (the macro contains its own minimal, upstream-reviewed unsafe).
    esp_alloc::heap_allocator!(size: HEAP_SIZE);

    // esp-rtos scheduler start: provides both the embassy time driver /
    // executor integration (`embassy` feature) and the RTOS scheduler that
    // esp-radio 0.18 requires (`esp-radio` feature). Replaces the former
    // `esp_hal_embassy::init` — esp-hal-embassy is dead upstream past
    // esp-hal 1.0.x (see Cargo.toml's rationale block).
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // esp-radio's BLE controller: `BleConnector` wraps the BT peripheral as
    // a `bt-hci` `Controller` impl for `trouble-host` to drive (wrapped in
    // `ExternalController` inside `ble_task`). In esp-radio 0.18 there is
    // no separate `esp_radio::init` step — the scheduler comes from
    // `esp_rtos::start` above, per the upstream bas_peripheral example.
    let ble_controller =
        esp_radio::ble::controller::BleConnector::new(peripherals.BT, Default::default())
            .expect("BLE controller init");

    // Two RMT channels, one per chain. 80 MHz with divider 1 gives 12.5 ns
    // ticks, which resolves every WS2812 interval exactly.
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

    // embassy-executor 0.10 reshaped this: `#[task]` functions now return
    // `Result<SpawnToken, SpawnError>` and `Spawner::spawn` takes the token and
    // returns `()`. The only failure is the task's pool being exhausted, which
    // for these single-instance tasks would be a bug here, not a runtime
    // condition worth handling.
    spawner.spawn(ble_task(ble_controller).expect("ble_task token"));
    // The render loop needs USB-detect: it is how the device distinguishes "no
    // daemon running" (a host is there, so amber, fixable) from "no host at all"
    // (aurora, just a fact). A pull-up because the line is driven low when VBUS
    // is present on this board.
    spawner.spawn(
        led_render_task(
            per_key_leds,
            underglow_leds,
            input_pin(peripherals.GPIO42, esp_hal::gpio::Pull::Up),
        )
        .expect("led_render_task token"),
    );
    // Power the upper PCB before anything tries to drive it. The LEDs live up
    // there; until this runs they have no supply, and the SPI writes go
    // nowhere visible. Levels are the vendor's, not a guess — see
    // `pins::TOP_BOARD_POWER`.
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
    // Deliberately leaked. `Output` is a guard: dropping it releases the GPIO,
    // and `main` returns as soon as the tasks are spawned — which would cut
    // power to the upper board before a single LED frame is drawn. That is
    // exactly the failure this chased for hours, with both SPI and RMT
    // faithfully clocking data into an unpowered board.
    core::mem::forget(_top_board_power);
    esp_println::println!("top board powered (held)");

    // ADC1 specifically: ADC2 is unusable while the radio is running, and this
    // firmware keeps BLE up continuously.
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
            // Drive lines idle low; a scan asserts one at a time.
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
            // Sense lines: pull-DOWN, per the vendor firmware. A pressed key
            // pulls them up toward the asserted drive line.
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
    // Display-mode commands over USB-Serial-JTAG. Only the RX half is used;
    // esp-println owns TX through raw register writes, so the TX guard is leaked
    // rather than dropped — dropping it would silence every log line.
    let (serial_rx, serial_tx) =
        esp_hal::usb_serial_jtag::UsbSerialJtag::new(peripherals.USB_DEVICE).split();
    core::mem::forget(serial_tx);
    spawner.spawn(serial_command_task(serial_rx).expect("serial_command_task token"));

    spawner.spawn(battery_task().expect("battery_task token"));
    spawner.spawn(
        power_task(input_pin(peripherals.GPIO2, esp_hal::gpio::Pull::Up))
            .expect("power_task token"),
    );
}

#[embassy_executor::task]
async fn ble_task(controller: esp_radio::ble::controller::BleConnector<'static>) {
    // See `ble.rs` for the (unverified) TrouBLE GATT server sketch. In the
    // real implementation this owns the `HostResources`/`GattServer`,
    // reads `LED_FRAME_CHANNEL` -> forwards to `led_render_task` via a
    // second channel (or shares `LED_FRAME_CHANNEL` as the single source
    // of truth — TBD once the attribute-write callback shape is known),
    // and drains `INPUT_EVENT_CHANNEL` / `BATTERY_CHANNEL` to notify.
    let _ = controller;
    loop {
        embassy_time::Timer::after_secs(3600).await; // placeholder idle loop
    }
}

/// The rear power button: short press wakes, ~2 s powers off, a long hold is
/// the bootloader escape hatch.
///
/// The gesture recognition is host-tested in `openmicro_effects::power`; all
/// this task owes it is a debounced level and a clock.
#[embassy_executor::task]
async fn power_task(button: esp_hal::gpio::Input<'static>) {
    use openmicro_effects::power::{PowerAction, PowerButton};

    let mut gesture = PowerButton::new();
    let start = embassy_time::Instant::now();
    loop {
        // Active low: the button shorts a pulled-up line to ground. INFERRED —
        // the vendor firmware only tells us the pin is an any-edge input, not
        // its rest level, so if the device powers itself off the instant it
        // boots, this is the polarity to flip.
        let pressed = button.is_low();
        let now_ms = start.elapsed().as_millis() as u32;

        match gesture.update(pressed, now_ms) {
            PowerAction::EnterBootloader => bootloader::reboot_to_bootloader(),
            PowerAction::PowerOff => {
                // NEXT: blank both LED chains, then enter deep sleep with the
                // button as the wake source.
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
    // A Ticker, not `Timer::after` at the end of the loop. `after` sleeps for a
    // fixed time *after* the work finishes, so the frame interval is 16 ms plus
    // however long rendering took — and a blocking USB-serial write or a longer
    // RMT transfer then shows up as a visible hitch. A ticker holds the phase.
    let mut ticker =
        embassy_time::Ticker::every(embassy_time::Duration::from_millis(leds::RENDER_PERIOD_MS));
    let start = embassy_time::Instant::now();
    let mut last_beat_ms = 0u32;
    let mut asleep = false;
    let mut last_link: Option<Link> = None;
    let mut last_demo_index: Option<usize> = None;
    // The frame the host last sent, held so the animation keeps running between
    // heartbeats instead of freezing for a second and a half at a time.
    let mut held = LedFrame::BLANK;

    loop {
        let t_ms = start.elapsed().as_millis() as u32;

        if display_mode() == MODE_PROBE {
            // Three fixed chain positions in three unmistakable colours, held
            // still. Whichever physical row the blue one is in tells us which
            // end of the chain is which, and that is the only thing standing
            // between `LED_FOR_KEY` being identity and being correct.
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
            // Bring-up aid for `layout::LED_FOR_KEY`: light one chain position
            // at a time and log it, so the physical order can be read off the
            // device and written into the table. Walks the ring in step.
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

        // Drain any frame the host sent. Taking the newest and dropping the rest
        // is right for a display: an older frame is never worth showing.
        while let Ok(frame) = LED_FRAME_CHANNEL.try_receive() {
            held = frame;
            LAST_FRAME_MS.store(t_ms, core::sync::atomic::Ordering::Relaxed);
        }

        let idle_for = t_ms.wrapping_sub(LAST_ACTIVITY_MS.load(core::sync::atomic::Ordering::Relaxed));
        let since_frame =
            t_ms.wrapping_sub(LAST_FRAME_MS.load(core::sync::atomic::Ordering::Relaxed));
        // Active low: VBUS-present pulls this line down through a divider on
        // this board. INFERRED — if the device thinks it is offline while
        // plugged in, this is the polarity to flip.
        let host_attached = usb_detect.is_low();
        let link = status::link_state(host_attached, since_frame);

        if openmicro_effects::startup::is_running(t_ms) {
            // The boot animation doubles as a wiring test: a sweep makes a
            // wrong chain length, order or colour order obvious, where a static
            // colour would hide all three.
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
            // Asleep: hold everything dark until something happens. Blanking
            // repeatedly is harmless — each frame is identical — and it keeps
            // the wake path a single comparison.
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
                // The daemon is driving: show what it sent, animated locally.
                Link::Live => {
                    keys.render(&held, t_ms);
                    glow.render(&held.glow, t_ms);
                }
                // Nobody is driving. The keys have nothing to say — no agent
                // state means no agent colours — but the ring must still show
                // *something*, because a dark board is indistinguishable from a
                // broken one.
                Link::NoDaemon | Link::Offline => {
                    keys.blank();
                    glow.render_link(link, LOCAL_BRIGHTNESS, t_ms);
                }
            }
        }

        // Heartbeat. USB-Serial-JTAG throws away anything written while no
        // host is listening, so a one-shot boot message is invisible unless a
        // monitor happened to be attached at exactly the right moment. A
        // periodic line makes "is it alive?" answerable at any time.
        if t_ms.wrapping_sub(last_beat_ms) >= 2000 {
            last_beat_ms = t_ms;
            esp_println::println!("alive t={}ms link={:?}", t_ms, link);
        }

        ticker.next().await;
    }
}

/// How long the device may sit untouched before the LEDs are blanked.
///
/// WS2812s held at a constant colour for days is how panels develop uneven
/// ageing, and this is a device that lives on a desk. Any input wakes it.
const LED_SLEEP_MS: u32 = 5 * 60 * 1000;

/// Milliseconds since boot when the device was last touched. Written by the
/// input and power tasks, read by the render loop.
static LAST_ACTIVITY_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Milliseconds since boot when the host last sent a frame.
///
/// Starts at zero, which reads as "a frame arrived at t=0" — but the boot
/// animation covers the first `startup::DURATION_MS`, and by the time it ends
/// the gap already exceeds `DAEMON_TIMEOUT_MS`, so the device correctly settles
/// into its own animation if nothing is talking to it.
static LAST_FRAME_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Record activity, waking the LEDs.
fn touch_activity(now_ms: u32) {
    LAST_ACTIVITY_MS.store(now_ms, core::sync::atomic::Ordering::Relaxed);
}

/// Brightness for the animations the device runs on its own.
///
/// Not the user's configured brightness: that arrives with a frame, and these
/// are exactly the states where no frame is arriving. Set moderate — these run
/// unattended, possibly for days, and they are ambient rather than urgent.
const LOCAL_BRIGHTNESS: u8 = 255;

/// What the display is doing. One of the `MODE_*` values below.
///
/// Runtime state rather than a build flag, so the TUI can switch it on a device
/// that is already flashed — asking someone to rebuild firmware to see a demo is
/// not a feature. Set over USB-Serial-JTAG; see `serial_command_task`.
static DISPLAY_MODE: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(INITIAL_DISPLAY_MODE);

/// Show real state: host frames, or the local fallback animations.
pub const MODE_NORMAL: u8 = 0;
/// Walk every state the board can show. See `openmicro_effects::demo`.
pub const MODE_DEMO: u8 = 1;
/// Light one chain position at a time, logging its index, to establish
/// `layout::LED_FOR_KEY`.
pub const MODE_IDENTIFY: u8 = 2;
/// Light three known chain positions in three fixed colours, and hold them.
///
/// Faster than [`MODE_IDENTIFY`] for the question that actually matters — which
/// end of the chain is the top of the board — because it can be answered by
/// looking once instead of watching thirteen steps go by.
pub const MODE_PROBE: u8 = 3;

/// Mode at boot. The env flags only seed it; the serial commands are the real
/// interface, and a reset always returns to normal.
const INITIAL_DISPLAY_MODE: u8 = if option_env!("OPENMICRO_DEMO").is_some() {
    MODE_DEMO
} else if option_env!("OPENMICRO_IDENTIFY").is_some() {
    MODE_IDENTIFY
} else {
    MODE_NORMAL
};

fn display_mode() -> u8 {
    DISPLAY_MODE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Apply a single-byte display-mode command.
///
/// Whitespace is ignored so a line-buffered terminal, which sends a trailing
/// newline, does not look like an unknown command.
fn handle_serial_command(byte: u8) {
    let mode = match byte {
        b'n' | b'N' => MODE_NORMAL,
        b'd' | b'D' => MODE_DEMO,
        b'i' | b'I' => MODE_IDENTIFY,
        b'p' | b'P' => MODE_PROBE,
        b'\r' | b'\n' | b' ' | b'\t' => return,
        other => {
            esp_println::println!("mode: unknown command {:?} (want n, d or i)", other as char);
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

/// Read display-mode commands from USB-Serial-JTAG.
///
/// Only the RX half is taken. esp-println writes to the TX FIFO through raw
/// register access rather than through this driver, so the two coexist — but it
/// does mean the TX guard must not be dropped, or output stops.
#[embassy_executor::task]
async fn serial_command_task(
    mut rx: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, esp_hal::Blocking>,
) {
    esp_println::println!(
        "mode: send 'd' demo, 'i' identify, 'p' probe, 'n' normal"
    );
    loop {
        // Drain whatever arrived. `read_byte` is non-blocking, so this returns
        // promptly and the poll interval below governs latency.
        while let Ok(byte) = rx.read_byte() {
            handle_serial_command(byte);
        }
        // 50 ms is imperceptible for a mode switch and costs nothing.
        embassy_time::Timer::after_millis(50).await;
    }
}



/// How long each LED stays lit in identify mode.
const IDENTIFY_DWELL_MS: u32 = 1500;

/// The three chain positions [`MODE_PROBE`] lights: both ends and the middle.
const PROBE_FIRST: usize = 0;
const PROBE_MIDDLE: usize = pins::PER_KEY_LED_COUNT / 2;
const PROBE_LAST: usize = pins::PER_KEY_LED_COUNT - 1;

/// `bool` to esp-hal's pin level.
fn level_of(high: bool) -> esp_hal::gpio::Level {
    if high {
        esp_hal::gpio::Level::High
    } else {
        esp_hal::gpio::Level::Low
    }
}

/// Configure one GPIO as an input with the given pull.
fn input_pin<'d>(
    pin: impl esp_hal::gpio::InputPin + 'd,
    pull: esp_hal::gpio::Pull,
) -> esp_hal::gpio::Input<'d> {
    esp_hal::gpio::Input::new(pin, esp_hal::gpio::InputConfig::default().with_pull(pull))
}

/// Everything `input_task` drives, moved in at spawn time.
///
/// Grouped into one struct because embassy tasks take their arguments by
/// value and eleven separate parameters would be unreadable.
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
    // Quadrature state, packed as (A << 1) | B, seeded from the pins so the
    // first real edge is not read as a phantom step.
    let mut prev_ab = ab_state(&io.encoder_a, &io.encoder_b);
    let mut sw_was_down = io.encoder_sw.is_low();
    let mut last_joystick_ms = 0u32;

    loop {
        let now_ms = start.elapsed().as_millis() as u32;

        // Scan: assert one drive line at a time and read every sense line.
        // The polarity is the vendor's, and it is backwards from the usual
        // keyboard convention — sense lines are pulled DOWN, so a pressed key
        // reads HIGH while its drive line is high.
        let mut raw = [[false; pins::MATRIX_SENSE_COUNT]; pins::MATRIX_DRIVE_COUNT];
        for (d, drive) in io.drive.iter_mut().enumerate() {
            drive.set_high();
            // Let the line settle before sampling: with a pull-down and any
            // trace capacitance the first read after a transition is a lie.
            embassy_time::Timer::after_micros(20).await;
            for (s, sense) in io.sense.iter().enumerate() {
                raw[d][s] = sense.is_high() == pins::MATRIX_ACTIVE_HIGH;
            }
            drive.set_low();
        }
        matrix.debounce(&raw, now_ms, |event| {
            // Only real events count as activity; the scan itself runs
            // continuously and would otherwise keep the LEDs awake forever.
            touch_activity(now_ms);
            // Logged as well as queued: until the BLE link exists, the serial
            // console is the only way to see that a key press was detected —
            // and it is what identifies the key-ID map.
            log::info!("input: {:?}", event);
            let _ = INPUT_EVENT_CHANNEL.try_send(event);
        });

        // Encoder: decode every quadrature transition, not just detents, so a
        // fast twist does not drop steps.
        let ab = ab_state(&io.encoder_a, &io.encoder_b);
        if ab != prev_ab {
            let step = input::encoder_step(prev_ab, ab);
            prev_ab = ab;
            if step != 0 {
                touch_activity(now_ms);
                let _ = INPUT_EVENT_CHANNEL.try_send(InputEvent::Encoder { delta: step });
            }
        }

        // Encoder press. Active low: the pin carries a pull-up and the switch
        // shorts it to ground.
        let sw_down = io.encoder_sw.is_low();
        if sw_down != sw_was_down {
            sw_was_down = sw_down;
            touch_activity(now_ms);
            let _ = INPUT_EVENT_CHANNEL.try_send(InputEvent::Key {
                id: pins::ENCODER_PRESS_KEY_ID,
                pressed: sw_down,
            });
        }

        // Joystick, sampled once every few scans — it is a menu selector, not
        // a pointer, so there is nothing to gain from 500 Hz.
        if now_ms.wrapping_sub(last_joystick_ms) >= JOYSTICK_PERIOD_MS {
            last_joystick_ms = now_ms;
            let x_raw = io.adc.read_blocking(&mut io.joy_x);
            let y_raw = io.adc.read_blocking(&mut io.joy_y);
            // The vendor firmware reports X inverted; match it so the host sees
            // the same orientation whichever firmware is running.
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

/// 12-bit ADC full scale, and the nominal centre of an un-deflected stick.
const ADC_MAX: u16 = 4095;
const ADC_CENTRE: u16 = 2048;
/// Deflection required before a direction is reported. Not calibrated against
/// a real stick — this is a first guess and wants tuning on hardware.
const JOYSTICK_DEADZONE: u16 = 700;
/// How often the stick is sampled.
const JOYSTICK_PERIOD_MS: u32 = 40;

/// Pack the encoder's two phases into the 2-bit code `input::encoder_step`
/// expects.
fn ab_state(a: &esp_hal::gpio::Input<'static>, b: &esp_hal::gpio::Input<'static>) -> u8 {
    ((a.is_high() as u8) << 1) | (b.is_high() as u8)
}

#[embassy_executor::task]
async fn battery_task() {
    // There is no analog battery pin to sample: the board carries a Maxim
    // MAX77972 combined charger and fuel gauge on I2C, and that is where the
    // host protocol's percentage and charging flag come from.
    //
    // BLOCKED: the bus pins are the one thing the vendor firmware would not
    // give up statically — it reaches the chip through Arduino's `Wire`, whose
    // `begin(sda, scl, freq)` is dispatched virtually. Candidates are
    // `pins::BATTERY_I2C_CANDIDATES`; an I2C scan on a real device for
    // `pins::BATTERY_I2C_ADDR` settles it in minutes. Until then this reports
    // nothing rather than inventing a reading.
    loop {
        embassy_time::Timer::after_secs(30).await;
    }
}

/// Static allocation cell placeholder for structures the real
/// implementation will need to `'static`-promote (e.g. `HostResources`).
/// Kept here as a visible reminder rather than left implicit.
static _RESOURCES_CELL: StaticCell<()> = StaticCell::new();
