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
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
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

/// Adapts esp-hal's blocking SPI master to `leds::SpiOut`.
struct SpiWriter(esp_hal::spi::master::Spi<'static, esp_hal::Blocking>);

impl leds::SpiOut for SpiWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), ()> {
        embedded_hal::spi::SpiBus::write(&mut self.0, bytes).map_err(|_| ())
    }
}

/// Build one WS2812 SPI host: MOSI is the LED data line, and the clock is
/// fixed by the bit encoding (see `openmicro_effects::ws2812`).
///
/// No MISO, no chip select: a WS2812 chain is write-only and has no select
/// line — the data pin is the entire bus.
macro_rules! ws2812_spi {
    ($peri:expr, $pin:expr) => {
        SpiWriter(
            esp_hal::spi::master::Spi::new(
                $peri,
                esp_hal::spi::master::Config::default()
                    .with_frequency(esp_hal::time::Rate::from_hz(
                        openmicro_effects::ws2812::SPI_HZ,
                    ))
                    .with_mode(esp_hal::spi::Mode::_0),
            )
            .expect("SPI init")
            .with_mosi($pin),
        )
    };
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger_from_env();

    // Clear the force-download-boot bit first thing. It survives a reset — and
    // on the Pro, whose RTC domain is battery-backed, it survives losing USB
    // power too — so a device that was put into download mode once would
    // otherwise keep going back there on every boot.
    bootloader::clear_force_download();

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

    // embassy-executor 0.10 reshaped this: `#[task]` functions now return
    // `Result<SpawnToken, SpawnError>` and `Spawner::spawn` takes the token and
    // returns `()`. The only failure is the task's pool being exhausted, which
    // for these single-instance tasks would be a bug here, not a runtime
    // condition worth handling.
    spawner.spawn(ble_task(ble_controller).expect("ble_task token"));
    spawner.spawn(
        led_render_task(
            ws2812_spi!(peripherals.SPI2, peripherals.GPIO7),
            ws2812_spi!(peripherals.SPI3, peripherals.GPIO6),
        )
        .expect("led_render_task token"),
    );
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
            PowerAction::Wake | PowerAction::None => {}
        }
        embassy_time::Timer::after_millis(10).await;
    }
}

#[embassy_executor::task]
async fn led_render_task(per_key: SpiWriter, underglow: SpiWriter) {
    let mut keys = leds::PerKeyChain::new(per_key);
    let mut glow = leds::UnderglowChain::new(underglow);
    let start = embassy_time::Instant::now();

    loop {
        let t_ms = start.elapsed().as_millis() as u32;

        if openmicro_effects::startup::is_running(t_ms) {
            // The boot animation doubles as a wiring test: a sweep makes a
            // wrong chain length, order or colour order obvious, where a static
            // colour would hide all three.
            keys.render_startup(t_ms);
            glow.render_startup(t_ms);
        } else {
            // NEXT: render the latest `LedFrame` from the BLE task here. Until
            // that channel is wired, the keys stay dark and the underglow
            // breathes — which is at least honest about the device being alive
            // and simply having nothing to show.
            glow.render_idle(t_ms);
        }

        embassy_time::Timer::after_millis(leds::RENDER_PERIOD_MS).await;
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
            let _ = INPUT_EVENT_CHANNEL.try_send(event);
        });

        // Encoder: decode every quadrature transition, not just detents, so a
        // fast twist does not drop steps.
        let ab = ab_state(&io.encoder_a, &io.encoder_b);
        if ab != prev_ab {
            let step = input::encoder_step(prev_ab, ab);
            prev_ab = ab;
            if step != 0 {
                let _ = INPUT_EVENT_CHANNEL.try_send(InputEvent::Encoder { delta: step });
            }
        }

        // Encoder press. Active low: the pin carries a pull-up and the switch
        // shorts it to ground.
        let sw_down = io.encoder_sw.is_low();
        if sw_down != sw_was_down {
            sw_was_down = sw_down;
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
