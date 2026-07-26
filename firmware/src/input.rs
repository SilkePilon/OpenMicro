use openmicro_proto::InputEvent;

use crate::pins;

pub const DEBOUNCE_MS: u32 = 10;

pub struct MatrixState {
    stable: [[bool; pins::MATRIX_SENSE_COUNT]; pins::MATRIX_DRIVE_COUNT],
    pending_since_ms: [[u32; pins::MATRIX_SENSE_COUNT]; pins::MATRIX_DRIVE_COUNT],
}

impl MatrixState {
    pub fn new() -> Self {
        Self {
            stable: [[false; pins::MATRIX_SENSE_COUNT]; pins::MATRIX_DRIVE_COUNT],
            pending_since_ms: [[0; pins::MATRIX_SENSE_COUNT]; pins::MATRIX_DRIVE_COUNT],
        }
    }

    pub fn debounce(
        &mut self,
        raw: &[[bool; pins::MATRIX_SENSE_COUNT]; pins::MATRIX_DRIVE_COUNT],
        now_ms: u32,
        mut emit: impl FnMut(InputEvent),
    ) {
        for row in 0..pins::MATRIX_DRIVE_COUNT {
            for col in 0..pins::MATRIX_SENSE_COUNT {
                let raw_pressed = raw[row][col];
                if raw_pressed != self.stable[row][col] {
                    if self.pending_since_ms[row][col] == 0 {
                        self.pending_since_ms[row][col] = now_ms.max(1);
                    } else if now_ms.wrapping_sub(self.pending_since_ms[row][col]) >= DEBOUNCE_MS {
                        self.stable[row][col] = raw_pressed;
                        self.pending_since_ms[row][col] = 0;
                        let id = (row * pins::MATRIX_SENSE_COUNT + col) as u8;
                        emit(InputEvent::Key {
                            id,
                            pressed: raw_pressed,
                        });
                    }
                } else {
                    self.pending_since_ms[row][col] = 0;
                }
            }
        }
    }
}

impl Default for MatrixState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn encoder_step(prev_ab: u8, now_ab: u8) -> i8 {
    match (prev_ab, now_ab) {
        (0b00, 0b01) | (0b01, 0b11) | (0b11, 0b10) | (0b10, 0b00) => 1,
        (0b00, 0b10) | (0b10, 0b11) | (0b11, 0b01) | (0b01, 0b00) => -1,
        _ => 0,
    }
}

pub fn joystick_to_sector(x: u16, y: u16, center: u16, deadzone: u16) -> Option<InputEvent> {
    let dx = x as i32 - center as i32;
    let dy = y as i32 - center as i32;
    if dx.unsigned_abs() < deadzone as u32 && dy.unsigned_abs() < deadzone as u32 {
        return None;
    }
    let (adx, ady) = (dx.unsigned_abs(), dy.unsigned_abs());
    let is_diagonal = |small: u32, large: u32| small * 64 >= large * 27;
    let sector: u8 = match (dx >= 0, dy >= 0) {
        (true, false) => {
            if is_diagonal(adx.min(ady), adx.max(ady)) {
                1
            } else if adx > ady {
                2
            } else {
                0
            }
        }
        (true, true) => {
            if is_diagonal(adx.min(ady), adx.max(ady)) {
                3
            } else if adx > ady {
                2
            } else {
                4
            }
        }
        (false, true) => {
            if is_diagonal(adx.min(ady), adx.max(ady)) {
                5
            } else if adx > ady {
                6
            } else {
                4
            }
        }
        (false, false) => {
            if is_diagonal(adx.min(ady), adx.max(ady)) {
                7
            } else if adx > ady {
                6
            } else {
                0
            }
        }
    };
    Some(InputEvent::Joystick { dir: sector })
}
