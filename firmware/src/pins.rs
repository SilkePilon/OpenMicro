//! Single source of truth for every physical GPIO the firmware needs.
//!
//! **None of the pin numbers below are known.** The Creator Micro 2 is an
//! ESP32-S3 board with no public schematic; the host-side protocol never
//! surfaces physical GPIOs (it only ever sees logical key IDs / encoder
//! deltas / polar joystick coordinates), so there is nothing to derive them
//! from in software. See
//! `docs/hardware/creator-micro-2-pinout-research.md` for the full writeup.
//!
//! Every constant below is a `// TODO(pinout):` placeholder grouped by
//! subsystem so that, once the pins are recovered (open the case and
//! continuity-test the PCB, or `esptool read-flash` + disassemble the stock
//! firmware — see the firmware README), this is the *only* file that needs
//! editing. Nothing outside this file should hardcode a `GpioN` pin number.
//!
//! Placeholder values use `esp_hal::gpio::GpioPin<0>` etc. as syntactically
//! valid stand-ins so the rest of the crate can reference a concrete type;
//! they are all TODO and must not be trusted.

// ---------------------------------------------------------------------
// 1. Key matrix (13 mechanical switches)
// ---------------------------------------------------------------------
// TODO(pinout): confirm matrix vs. direct-GPIO wiring, then fill in row/col
// pins and diode direction. The v1 (atmega32u4) board used a 4x4 matrix
// with two extra direct-read pins for one row; the Micro 2's ESP32-S3
// layout is NOT confirmed to match — do not assume it does.
pub const KEY_MATRIX_ROW_COUNT: usize = 4; // TODO(pinout): verify
pub const KEY_MATRIX_COL_COUNT: usize = 4; // TODO(pinout): verify
                                           // TODO(pinout): pub const KEY_MATRIX_ROW_PINS: [u8; KEY_MATRIX_ROW_COUNT] = [..];
                                           // TODO(pinout): pub const KEY_MATRIX_COL_PINS: [u8; KEY_MATRIX_COL_COUNT] = [..];
                                           // TODO(pinout): diode direction (COL2ROW vs ROW2COL) — unknown for Micro 2.

// ---------------------------------------------------------------------
// 2. Per-key RGB + underglow (WS2812/SK6812 chain over RMT)
// ---------------------------------------------------------------------
// TODO(pinout): single-wire RGB data GPIO. Chip type (WS2812 vs SK6812) is
// also unconfirmed for the Micro 2 — WS2812 timings are used as the default
// in `leds.rs` until proven otherwise.
// TODO(pinout): pub const LED_DATA_GPIO: u8 = 0;
/// Total LEDs in the chain: `SLOT_COUNT` (6, agent keys) + the remaining
/// mechanical keys + underglow. Exact count/order is unconfirmed.
// TODO(pinout): confirm total count and the logical-slot -> chain-index map.
pub const LED_COUNT: usize = openmicro_proto::SLOT_COUNT + 8; // TODO(pinout): placeholder guess

// ---------------------------------------------------------------------
// 3. Rotary encoder (1x, with press)
// ---------------------------------------------------------------------
// TODO(pinout): pub const ENCODER_PIN_A: u8 = 0;
// TODO(pinout): pub const ENCODER_PIN_B: u8 = 0;
// TODO(pinout): pub const ENCODER_SWITCH_PIN: u8 = 0;

// ---------------------------------------------------------------------
// 4. Planar joystick (1x, ADC X/Y -> polar angle/distance)
// ---------------------------------------------------------------------
// TODO(pinout): pub const JOYSTICK_ADC_X_PIN: u8 = 0;
// TODO(pinout): pub const JOYSTICK_ADC_Y_PIN: u8 = 0;
// TODO(pinout): joystick button pin, if one exists in hardware (unconfirmed).

// ---------------------------------------------------------------------
// 5. Capacitive touch sensor (1x)
// ---------------------------------------------------------------------
// TODO(pinout): pub const TOUCH_PIN: u8 = 0;

// ---------------------------------------------------------------------
// 6. Battery sense / charger (PRO only, 2100 mAh)
// ---------------------------------------------------------------------
// TODO(pinout): battery could be an ADC pin or an I2C fuel-gauge (e.g.
// MAX17048-style) — which one is unconfirmed. Fill in whichever applies.
// TODO(pinout): pub const BATTERY_ADC_PIN: u8 = 0;
// TODO(pinout): pub const BATTERY_I2C_SDA_PIN: u8 = 0;
// TODO(pinout): pub const BATTERY_I2C_SCL_PIN: u8 = 0;

// ---------------------------------------------------------------------
// 7. Flash size / strapping
// ---------------------------------------------------------------------
// TODO(pinout): total flash size is only "likely >= 4 MB" (implied by the
// standard partition layout in the research doc); confirm with `esptool
// flash_id` once the device is in bootloader mode. Strapping pins are the
// ESP32-S3 defaults unless the board overrides them (unconfirmed).
