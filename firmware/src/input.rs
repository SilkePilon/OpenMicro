//! Input scanning: key matrix, rotary encoder, joystick.
//!
//! Structural skeleton only — see the crate-level docs in `main.rs`. All
//! GPIOs are `// TODO(pinout):` per `pins.rs`; this module is written
//! against the shape of `esp-hal`'s (unstable) GPIO/ADC APIs but is
//! UNVERIFIED (no Xtensa toolchain on this machine, see the firmware
//! README).

use openmicro_proto::InputEvent;

use crate::pins;

/// Debounce window for the key matrix. 8-12ms is typical for mechanical
/// switches; not derived from real hardware measurements.
pub const DEBOUNCE_MS: u32 = 10;

/// One row/col matrix scan cycle's worth of state, used to debounce.
pub struct MatrixState {
    // TODO(pinout): size against the real KEY_MATRIX_ROW_COUNT / COL_COUNT
    // once the wiring (matrix vs. direct-GPIO) is confirmed.
    stable: [[bool; pins::KEY_MATRIX_COL_COUNT]; pins::KEY_MATRIX_ROW_COUNT],
    pending_since_ms: [[u32; pins::KEY_MATRIX_COL_COUNT]; pins::KEY_MATRIX_ROW_COUNT],
}

impl MatrixState {
    pub fn new() -> Self {
        Self {
            stable: [[false; pins::KEY_MATRIX_COL_COUNT]; pins::KEY_MATRIX_ROW_COUNT],
            pending_since_ms: [[0; pins::KEY_MATRIX_COL_COUNT]; pins::KEY_MATRIX_ROW_COUNT],
        }
    }

    /// Feed one raw (pre-debounce) scan of the matrix at time `now_ms`,
    /// yielding any `InputEvent::Key` transitions whose state has been
    /// stable for `DEBOUNCE_MS`.
    ///
    /// Row/col GPIO drive + read (`// TODO(pinout):` row/col pins) happens
    /// in the caller (the embassy input task in `main.rs`); this is pure
    /// software debounce logic so it stays host-testable in spirit even
    /// though it currently lives in the embedded crate.
    pub fn debounce(
        &mut self,
        raw: &[[bool; pins::KEY_MATRIX_COL_COUNT]; pins::KEY_MATRIX_ROW_COUNT],
        now_ms: u32,
        mut emit: impl FnMut(InputEvent),
    ) {
        for row in 0..pins::KEY_MATRIX_ROW_COUNT {
            for col in 0..pins::KEY_MATRIX_COL_COUNT {
                let raw_pressed = raw[row][col];
                if raw_pressed != self.stable[row][col] {
                    if self.pending_since_ms[row][col] == 0 {
                        self.pending_since_ms[row][col] = now_ms.max(1);
                    } else if now_ms.wrapping_sub(self.pending_since_ms[row][col]) >= DEBOUNCE_MS {
                        self.stable[row][col] = raw_pressed;
                        self.pending_since_ms[row][col] = 0;
                        // TODO(pinout): key id mapping (row,col) -> logical
                        // key id (0..13) is unconfirmed; identity-ish
                        // placeholder below.
                        let id = (row * pins::KEY_MATRIX_COL_COUNT + col) as u8;
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

/// Rotary encoder quadrature decode. Call on every A/B edge interrupt (or a
/// fast poll loop); returns +-1 per detent, 0 for a non-detent edge.
///
/// `// TODO(pinout):` `ENCODER_PIN_A` / `ENCODER_PIN_B`.
pub fn encoder_step(prev_ab: u8, now_ab: u8) -> i8 {
    // Standard quadrature transition table (2-bit gray code).
    match (prev_ab, now_ab) {
        (0b00, 0b01) | (0b01, 0b11) | (0b11, 0b10) | (0b10, 0b00) => 1,
        (0b00, 0b10) | (0b10, 0b11) | (0b11, 0b01) | (0b01, 0b00) => -1,
        _ => 0,
    }
}

/// Convert raw joystick ADC X/Y (each 0..=4095 for the ESP32-S3's 12-bit
/// ADC) to the protocol's logical direction sector.
///
/// The wire protocol only has `InputEvent::Joystick { dir: u8 }` today
/// (see `openmicro-proto`); the research doc notes the real device exposes
/// polar angle/distance to a 7-slot radial menu upstream, so `dir` here is
/// treated as an 8-way sector index (`N, NE, E, SE, S, SW, W, NW`) as a
/// reasonable firmware-side approximation until the proto is extended to
/// carry angle+distance directly. `// TODO(pinout):` `JOYSTICK_ADC_X_PIN` /
/// `JOYSTICK_ADC_Y_PIN`, plus deadzone calibration once real ADC center/
/// range values are known.
pub fn joystick_to_sector(x: u16, y: u16, center: u16, deadzone: u16) -> Option<InputEvent> {
    let dx = x as i32 - center as i32;
    let dy = y as i32 - center as i32;
    if dx.unsigned_abs() < deadzone as u32 && dy.unsigned_abs() < deadzone as u32 {
        return None; // inside the deadzone: no event.
    }
    // 8-way sector without atan2/floats. Each 45-degree sector is picked by
    // comparing |dx| against |dy| scaled by a fixed-point tan(22.5-deg) =~
    // 0.4142 (approximated here as 27/64 =~ 0.4219, close enough for an 8-way
    // menu selection): if the smaller axis is under that fraction of the
    // larger one, the point is "axis-aligned" (N/E/S/W); otherwise it is
    // diagonal (NE/SE/SW/NW). Sector indices: 0=N, 1=NE, 2=E, 3=SE, 4=S,
    // 5=SW, 6=W, 7=NW (screen-style Y axis: dy > 0 means "down"/S).
    let (adx, ady) = (dx.unsigned_abs(), dy.unsigned_abs());
    let is_diagonal = |small: u32, large: u32| small * 64 >= large * 27;
    let sector: u8 = match (dx >= 0, dy >= 0) {
        (true, false) => {
            // NE quadrant (+x, -y)
            if is_diagonal(adx.min(ady), adx.max(ady)) {
                1 // NE
            } else if adx > ady {
                2 // E
            } else {
                0 // N
            }
        }
        (true, true) => {
            // SE quadrant (+x, +y)
            if is_diagonal(adx.min(ady), adx.max(ady)) {
                3 // SE
            } else if adx > ady {
                2 // E
            } else {
                4 // S
            }
        }
        (false, true) => {
            // SW quadrant (-x, +y)
            if is_diagonal(adx.min(ady), adx.max(ady)) {
                5 // SW
            } else if adx > ady {
                6 // W
            } else {
                4 // S
            }
        }
        (false, false) => {
            // NW quadrant (-x, -y)
            if is_diagonal(adx.min(ady), adx.max(ady)) {
                7 // NW
            } else if adx > ady {
                6 // W
            } else {
                0 // N
            }
        }
    };
    Some(InputEvent::Joystick { dir: sector })
}
