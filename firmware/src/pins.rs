//! Single source of truth for every physical GPIO the firmware needs.
//!
//! These numbers were recovered from Work Louder's own shipping firmware, not
//! guessed: the vendor publishes unencrypted merged images to
//! `worklouder/cm-v2-fw-releases`, and disassembling one puts every
//! `gpio_config()` pin bitmask next to an assert string naming its `PIN_*`
//! macro. The full derivation, with offsets and proofs, is in
//! `docs/hardware/creator-micro-2-pinout-findings.md`.
//!
//! Confidence is marked per group. Anything still unknown says so rather than
//! carrying a plausible-looking placeholder — a wrong pin here drives an output
//! into another output.
//!
//! Nothing outside this file should hardcode a GPIO number.

/// Pins that must never be repurposed on this board.
///
/// Kept as a real constant (and asserted against below) rather than a comment,
/// because "don't drive that one" is exactly the kind of note that gets lost.
/// GPIO19/20 are the native USB D-/D+ lines; 26..=32 are the module's SPI flash
/// and PSRAM.
pub const RESERVED_GPIOS: [u8; 9] = [19, 20, 26, 27, 28, 29, 30, 31, 32];

// ---------------------------------------------------------------------
// 1. Key matrix — CONFIRMED (4x4, 13 of 16 positions populated)
// ---------------------------------------------------------------------
// The stock firmware configures the drive lines as push-pull OUTPUTs and the
// sense lines as INPUTs with an internal pull-DOWN and an any-edge interrupt.
//
// That pull-down is the important detail: it inverts the usual keyboard
// convention. Current flows drive -> sense through a pressed key, so a key
// reads as pressed when its drive line is asserted HIGH and the sense line
// follows it up against the pull-down. Diodes are therefore anode-on-drive,
// cathode-on-sense (INFERRED from the electrical roles; worth a diode-test
// meter before trusting it).
pub const MATRIX_DRIVE_PINS: [u8; 4] = [46, 17, 40, 47];
pub const MATRIX_SENSE_PINS: [u8; 4] = [13, 5, 21, 1];
pub const MATRIX_DRIVE_COUNT: usize = MATRIX_DRIVE_PINS.len();
pub const MATRIX_SENSE_COUNT: usize = MATRIX_SENSE_PINS.len();

/// Mechanical switches actually fitted (3 of the 16 intersections are unused).
pub const KEY_COUNT: usize = 13;

/// Sense lines are asserted HIGH by a pressed key, not pulled low.
pub const MATRIX_ACTIVE_HIGH: bool = true;

// UNKNOWN: which physical key sits at each (drive, sense) intersection. The
// firmware's keymap table would give it, or pressing keys on the real device
// and watching which pair fires. Until then the scanner reports raw
// coordinates and `key_id` is the flattened index.

// ---------------------------------------------------------------------
// 2. RGB — CONFIRMED (WS2812 family, driven over SPI, two chains)
// ---------------------------------------------------------------------
// Not RMT: the stock firmware uses ESP-IDF `led_strip` in its SPI backend, and
// each chain sits on its own SPI host, so they are independent buses and must
// not be daisy-chained.
pub const PER_KEY_LED_GPIO: u8 = 7;
pub const PER_KEY_LED_COUNT: usize = 13;
pub const UNDERGLOW_LED_GPIO: u8 = 6;
pub const UNDERGLOW_LED_COUNT: usize = 8;

/// Every addressable LED on the board.
pub const LED_COUNT: usize = PER_KEY_LED_COUNT + UNDERGLOW_LED_COUNT;

// INFERRED (high): WS2812 family with GRB byte order — the `led_strip`
// default. If colours come out swapped on real hardware, this is the first
// thing to try changing.

// ---------------------------------------------------------------------
// 3. Layer / indicator LEDs — CONFIRMED (single colour, LEDC PWM)
// ---------------------------------------------------------------------
/// Three plain LEDs on PWM channels, separate from the RGB chains.
///
/// GPIO45 is a strapping pin (VDD_SPI). Driving it as an output after boot is
/// fine — it is only sampled at reset — but nothing may hold it during
/// power-up.
pub const LAYER_LED_PINS: [u8; 3] = [35, 45, 48];

// ---------------------------------------------------------------------
// 4. Rotary encoder — CONFIRMED
// ---------------------------------------------------------------------
/// Key id reported for a dial press.
///
/// The wire protocol has `Key`, `Encoder` and `Joystick` events but no
/// dedicated dial-press variant, and adding one would ripple through the
/// daemon's action router for an input whose behaviour is not designed yet. A
/// reserved id well clear of the 13 real keys carries it in the meantime.
pub const ENCODER_PRESS_KEY_ID: u8 = 0xFE;

pub const ENCODER_PIN_A: u8 = 12;
pub const ENCODER_PIN_B: u8 = 11;
pub const ENCODER_SWITCH_PIN: u8 = 4;

// ---------------------------------------------------------------------
// 5. Planar joystick — CONFIRMED (both axes on ADC1)
// ---------------------------------------------------------------------
// ADC1 matters: ADC2 is unusable while the radio is active, and this firmware
// runs BLE continuously.
pub const JOYSTICK_ADC_X_PIN: u8 = 9; // ADC1_CH8
pub const JOYSTICK_ADC_Y_PIN: u8 = 10; // ADC1_CH9

/// The stock firmware reports X as `4095 - raw`.
pub const JOYSTICK_X_INVERTED: bool = true;

// UNKNOWN: a dedicated joystick push-button. The vendor's sampler reads only
// X/Y, so the press is probably one of the 13 matrix keys or a deflection
// threshold rather than its own GPIO.

// ---------------------------------------------------------------------
// 6. Capacitive touch — CONFIRMED (external controller, NOT the ESP peripheral)
// ---------------------------------------------------------------------
/// Active-low interrupt line from an external touch controller.
///
/// Do not point the ESP32-S3 `touch_pad` peripheral at this: it is a plain
/// digital input, and the controller itself lives on I2C.
pub const TOUCH_IRQ_PIN: u8 = 14;
pub const TOUCH_ACTIVE_LOW: bool = true;

// ---------------------------------------------------------------------
// 7. Misc I/O — CONFIRMED
// ---------------------------------------------------------------------
pub const REAR_BUTTON_PIN: u8 = 2;
/// VBUS-present detect.
pub const USB_DETECT_PIN: u8 = 42;
/// Power gate for the upper PCB, with the levels the vendor firmware drives.
///
/// Read out of `wl_io::init_top_board_power_gpio` in the stock image: it
/// configures 36/37/38 as outputs (pin mask high word `0x70`) and then calls a
/// setter with `(37 = 1, 36 = 0, 38 = 1)`. Note that they are **not** all high
/// — driving 36 high would have been the obvious guess and is wrong.
///
/// Without this the upper board has no power, which is exactly what "the LEDs
/// never light up" looks like.
pub const TOP_BOARD_POWER: [(u8, bool); 3] = [(36, false), (37, true), (38, true)];

/// Charge enable. INFERRED (medium) — the store alignment around it was noisy
/// in the disassembly, so verify before driving it.
pub const CHARGE_ENABLE_PIN: u8 = 44;

// ---------------------------------------------------------------------
// 8. Battery / charger — chip CONFIRMED, bus pins UNKNOWN
// ---------------------------------------------------------------------
// There is **no analog battery-sense pin**. Battery state and charging are a
// Maxim MAX77972 combined charger + fuel gauge on I2C, which is where the
// host protocol's percentage and charging flag come from.
//
// The SDA/SCL GPIOs could not be recovered statically: the vendor firmware
// reaches the chip through Arduino's `Wire`, whose `begin(sda, scl, freq)` is
// dispatched virtually. They are none of the pins assigned above; the
// realistic candidates are 3, 8, 15, 16, 18, 33 and 34. An I2C scan on the
// real device settles it in minutes.
//
// Deliberately left as `None` rather than a guessed pair: driving the wrong
// two pins as an I2C bus is exactly the mistake this file exists to prevent.
pub const BATTERY_I2C_SDA_PIN: Option<u8> = None;
pub const BATTERY_I2C_SCL_PIN: Option<u8> = None;
/// 7-bit address the MAX77972 is expected to answer on.
pub const BATTERY_I2C_ADDR: u8 = 0x69;
/// Pins worth sweeping when scanning for the battery bus.
pub const BATTERY_I2C_CANDIDATES: [u8; 7] = [3, 8, 15, 16, 18, 33, 34];

// ---------------------------------------------------------------------
// 9. Flash — CONFIRMED from the vendor image header
// ---------------------------------------------------------------------
/// 16 MB, DIO, 80 MHz. The app partition sits at 0x10000.
pub const FLASH_SIZE_BYTES: u32 = 16 * 1024 * 1024;

/// Compile-time check that no pin we drive collides with a reserved one.
///
/// This runs at build time, so a future edit that puts, say, the encoder on a
/// flash pin fails the build instead of the hardware.
const _: () = {
    const DRIVEN: [u8; 20] = [
        MATRIX_DRIVE_PINS[0],
        MATRIX_DRIVE_PINS[1],
        MATRIX_DRIVE_PINS[2],
        MATRIX_DRIVE_PINS[3],
        MATRIX_SENSE_PINS[0],
        MATRIX_SENSE_PINS[1],
        MATRIX_SENSE_PINS[2],
        MATRIX_SENSE_PINS[3],
        PER_KEY_LED_GPIO,
        UNDERGLOW_LED_GPIO,
        LAYER_LED_PINS[0],
        LAYER_LED_PINS[1],
        LAYER_LED_PINS[2],
        ENCODER_PIN_A,
        ENCODER_PIN_B,
        ENCODER_SWITCH_PIN,
        JOYSTICK_ADC_X_PIN,
        JOYSTICK_ADC_Y_PIN,
        TOUCH_IRQ_PIN,
        REAR_BUTTON_PIN,
    ];

    let mut i = 0;
    while i < DRIVEN.len() {
        let mut r = 0;
        while r < RESERVED_GPIOS.len() {
            assert!(DRIVEN[i] != RESERVED_GPIOS[r], "pin collides with flash/PSRAM/USB");
            r += 1;
        }
        // And no pin may be claimed twice.
        let mut j = i + 1;
        while j < DRIVEN.len() {
            assert!(DRIVEN[i] != DRIVEN[j], "the same GPIO is assigned to two functions");
            j += 1;
        }
        i += 1;
    }
};
