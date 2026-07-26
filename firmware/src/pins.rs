pub const RESERVED_GPIOS: [u8; 9] = [19, 20, 26, 27, 28, 29, 30, 31, 32];

pub const MATRIX_DRIVE_PINS: [u8; 4] = [46, 17, 40, 47];
pub const MATRIX_SENSE_PINS: [u8; 4] = [13, 5, 21, 1];
pub const MATRIX_DRIVE_COUNT: usize = MATRIX_DRIVE_PINS.len();
pub const MATRIX_SENSE_COUNT: usize = MATRIX_SENSE_PINS.len();

pub const KEY_COUNT: usize = 13;

pub const MATRIX_ACTIVE_HIGH: bool = true;

pub const PER_KEY_LED_GPIO: u8 = 7;
pub const PER_KEY_LED_COUNT: usize = 13;
pub const UNDERGLOW_LED_GPIO: u8 = 6;
pub const UNDERGLOW_LED_COUNT: usize = 8;

pub const LED_COUNT: usize = PER_KEY_LED_COUNT + UNDERGLOW_LED_COUNT;

pub const LAYER_LED_PINS: [u8; 3] = [35, 45, 48];

pub const ENCODER_PRESS_KEY_ID: u8 = 0xFE;

pub const ENCODER_PIN_A: u8 = 12;
pub const ENCODER_PIN_B: u8 = 11;
pub const ENCODER_SWITCH_PIN: u8 = 4;

pub const JOYSTICK_ADC_X_PIN: u8 = 9;
pub const JOYSTICK_ADC_Y_PIN: u8 = 10;

pub const JOYSTICK_X_INVERTED: bool = true;

pub const TOUCH_IRQ_PIN: u8 = 14;
pub const TOUCH_ACTIVE_LOW: bool = true;

pub const REAR_BUTTON_PIN: u8 = 2;
pub const USB_DETECT_PIN: u8 = 42;
pub const TOP_BOARD_POWER: [(u8, bool); 3] = [(36, true), (37, false), (38, false)];

pub const CHARGE_ENABLE_PIN: u8 = 44;

pub const BATTERY_I2C_SDA_PIN: Option<u8> = None;
pub const BATTERY_I2C_SCL_PIN: Option<u8> = None;
pub const BATTERY_I2C_ADDR: u8 = 0x69;
pub const BATTERY_I2C_CANDIDATES: [u8; 7] = [3, 8, 15, 16, 18, 33, 34];

pub const FLASH_SIZE_BYTES: u32 = 16 * 1024 * 1024;

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
        let mut j = i + 1;
        while j < DRIVEN.len() {
            assert!(DRIVEN[i] != DRIVEN[j], "the same GPIO is assigned to two functions");
            j += 1;
        }
        i += 1;
    }
};
