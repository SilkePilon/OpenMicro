//! OpenMicro ESP32-S3 firmware — embedded skeleton.
//!
//! # Status: COMPILES ON CI, NEVER RUN ON HARDWARE
//!
//! This crate builds for `xtensa-esp32s3-none-elf` on CI
//! (`.github/workflows/firmware.yml`), against the version set pinned in
//! `Cargo.toml` (esp-hal 1.1.1 + esp-rtos 0.3.0 + esp-radio 0.18.0 +
//! trouble-host 0.6.0 — the same set as the upstream `esp-hal-v1.1.1`
//! `bas_peripheral` BLE example). Compiling is all it is proven to do: it
//! has never been flashed to or run on a device, and every GPIO is still a
//! `// TODO(pinout):` placeholder, so it cannot drive the real hardware
//! yet. `openmicro-effects`/`openmicro-proto` (the shared pieces this crate
//! calls into) are host-tested in the root workspace.
//!
//! # Why the GPIO pins are all placeholders
//!
//! See `pins.rs` and `docs/hardware/creator-micro-2-pinout-research.md`:
//! the Creator Micro 2's physical wiring is not publicly documented and the
//! host-side protocol never surfaces it, so every pin here is a
//! `// TODO(pinout):` constant.
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
//! - `battery_task`: samples battery state and updates the Battery Service
//!   characteristic via the BLE task.

#![no_std]
#![no_main]

mod ble;
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

    spawner.must_spawn(ble_task(ble_controller));
    spawner.must_spawn(led_render_task());
    spawner.must_spawn(input_task());
    spawner.must_spawn(battery_task());
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

#[embassy_executor::task]
async fn led_render_task() {
    // TODO(pinout): construct the real `leds::LedStrip` once LED_DATA_GPIO
    // and an RMT channel are available; render `LED_FRAME_CHANNEL`'s latest
    // value every `leds::RENDER_PERIOD_MS` using a free-running embassy
    // `Instant`-derived `t_ms` fed into `openmicro_effects::resolve`.
    loop {
        embassy_time::Timer::after_millis(leds::RENDER_PERIOD_MS).await;
    }
}

#[embassy_executor::task]
async fn input_task() {
    // TODO(pinout): drive the key matrix / read encoder & joystick GPIOs,
    // debounce via `input::MatrixState`, and push resulting `InputEvent`s
    // onto `INPUT_EVENT_CHANNEL`.
    loop {
        embassy_time::Timer::after_millis(5).await;
    }
}

#[embassy_executor::task]
async fn battery_task() {
    // TODO(pinout): sample `pins::BATTERY_ADC_PIN` (or the fuel-gauge I2C
    // pins, whichever the recovered pinout turns out to use) and push a
    // `Battery { pct, charging }` onto `BATTERY_CHANNEL` periodically.
    loop {
        embassy_time::Timer::after_secs(30).await;
    }
}

/// Static allocation cell placeholder for structures the real
/// implementation will need to `'static`-promote (e.g. `HostResources`).
/// Kept here as a visible reminder rather than left implicit.
static _RESOURCES_CELL: StaticCell<()> = StaticCell::new();
