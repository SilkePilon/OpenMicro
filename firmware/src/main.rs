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

    // TODO(pinout): LED RMT channel + LED_DATA_GPIO (see pins.rs / leds.rs).
    // TODO(pinout): key matrix row/col GPIOs, encoder A/B/press GPIOs,
    // joystick ADC X/Y GPIOs, battery sense pin (see pins.rs / input.rs).

    // embassy-executor 0.10 reshaped this: `#[task]` functions now return
    // `Result<SpawnToken, SpawnError>` and `Spawner::spawn` takes the token and
    // returns `()`. The only failure is the task's pool being exhausted, which
    // for these single-instance tasks would be a bug here, not a runtime
    // condition worth handling.
    spawner.spawn(ble_task(ble_controller).expect("ble_task token"));
    spawner.spawn(led_render_task().expect("led_render_task token"));
    spawner.spawn(input_task().expect("input_task token"));
    spawner.spawn(battery_task().expect("battery_task token"));
    spawner.spawn(power_task().expect("power_task token"));
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
async fn power_task() {
    use openmicro_effects::power::{PowerAction, PowerButton};

    let mut button = PowerButton::new();
    loop {
        // NEXT: read `pins::REAR_BUTTON_PIN` here. Until the HAL plumbing
        // lands the button always reads released, so no action ever fires —
        // which is the safe direction to be wrong in for something that can
        // power the device off.
        let pressed = false;
        let now_ms = embassy_time::Instant::now().as_millis() as u32;

        match button.update(pressed, now_ms) {
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
async fn led_render_task() {
    // The boot animation runs first, straight from `openmicro_effects::startup`
    // (host-tested): it doubles as a check that the chain length and colour
    // order are right, which a static colour would hide.
    //
    // NEXT: hand `leds::PerKeyChain` (GPIO7, 13 LEDs) and
    // `leds::UnderglowChain` (GPIO6, 8 LEDs) an `SpiOut` backed by esp-hal's
    // SPI master on their two hosts, then render `LED_FRAME_CHANNEL`'s latest
    // value every `leds::RENDER_PERIOD_MS` with a free-running embassy
    // `Instant`-derived `t_ms`. The bit encoding is already done and tested
    // (`openmicro_effects::ws2812`, 3.2 MHz, 4 SPI bits per LED bit); what is
    // missing is only the HAL plumbing, which cannot be validated without the
    // board in hand.
    loop {
        embassy_time::Timer::after_millis(leds::RENDER_PERIOD_MS).await;
    }
}

#[embassy_executor::task]
async fn input_task() {
    // NEXT: drive `pins::MATRIX_DRIVE_PINS` high one at a time and read
    // `pins::MATRIX_SENSE_PINS` (pull-down; a pressed key reads HIGH — the
    // opposite of the usual convention), debounce via `input::MatrixState`,
    // and push `InputEvent`s onto `INPUT_EVENT_CHANNEL`. The encoder is an
    // any-edge interrupt on `pins::ENCODER_PIN_A`/`B` through
    // `input::encoder_step`; the joystick is ADC1 on
    // `pins::JOYSTICK_ADC_X_PIN`/`Y` (invert X per
    // `pins::JOYSTICK_X_INVERTED`) through `input::joystick_to_sector`.
    loop {
        embassy_time::Timer::after_millis(5).await;
    }
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
