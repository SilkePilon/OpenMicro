//! WS2812 (NeoPixel-style) LED strip render task.
//!
//! **Not using `esp-hal-smartled`.** As of this writing `esp-hal-smartled`
//! 0.17.0 pins its `esp-hal` dependency to `~1.0` (i.e. `>=1.0.0, <1.1.0`,
//! confirmed from its published `Cargo.toml`), while `esp-radio` 0.18.0 (our
//! BLE stack, see `ble.rs`) pins `esp-hal` to `~1.1.0-rc.0` (`>=1.1.0-rc.0`).
//! Those two ranges do not overlap on a real released `esp-hal` version, so
//! `esp-hal-smartled` cannot currently sit in the same dependency graph as
//! `esp-radio` on top-of-tree `esp-hal`. Rather than downgrade the whole
//! firmware to a stale `esp-hal` 1.0.x to keep `esp-hal-smartled`, we drive
//! the WS2812 chain directly over `esp_hal::rmt` (the same peripheral
//! `esp-hal-smartled` itself wraps) — it is part of `esp-hal` proper so it
//! never has an independent version to skew. See the firmware report for
//! the full version-pinning rationale.
//!
//! This module is a structural skeleton only (see the crate-level docs in
//! `main.rs`): it is not compiled here (no Xtensa toolchain on this
//! machine) and the RMT item-encoding calls below are written to the
//! documented shape of `esp_hal::rmt` but are UNVERIFIED.

use openmicro_effects::resolve;
use openmicro_proto::{LedFrame, Rgb};

use crate::pins;

/// Render tick period. ~60 Hz gives a visibly smooth Breath/Pulse/Rainbow
/// without saturating the RMT peripheral or the BLE link.
pub const RENDER_PERIOD_MS: u64 = 16;

/// WS2812 bit timings (T0H/T0L/T1H/T1L), nanoseconds, per the common
/// WS2812B datasheet figures. `// TODO(pinout):` also covers "confirm the
/// LED chip is actually WS2812 and not SK6812" — the research doc could not
/// confirm the chip type, so these timings are a best-effort default.
mod ws2812_timing {
    pub const T0H_NS: u32 = 400;
    pub const T0L_NS: u32 = 850;
    pub const T1H_NS: u32 = 800;
    pub const T1L_NS: u32 = 450;
    pub const RESET_LOW_NS: u32 = 50_000;
}

/// Maps a logical render slot (0..SLOT_COUNT agent-key slots, then
/// underglow/other keys) to a position in the physical LED chain.
/// `// TODO(pinout):` the actual chain order is unconfirmed — this identity
/// mapping is a placeholder.
fn slot_to_chain_index(slot: usize) -> usize {
    slot // TODO(pinout): replace with the real physical chain order.
}

/// Owns the RMT TX channel wired to the LED data line and pushes rendered
/// frames out over it.
///
/// `Tx` is left generic over the concrete `esp-hal` RMT channel type rather
/// than naming it directly, since that type is parameterized by peripheral
/// instance/pin in ways that cannot be pinned down without the real
/// `LED_DATA_GPIO` from `pins.rs`.
pub struct LedStrip<Tx> {
    channel: Tx,
    buf: [Rgb; pins::LED_COUNT],
}

impl<Tx> LedStrip<Tx>
where
    // TODO: once `pins::LED_DATA_GPIO` is known, replace this bound with
    // the concrete `esp_hal::rmt::TxChannel` (or channel-creator) type for
    // that GPIO, per the esp-hal-smartled `hello_rgb.rs` example shape.
    Tx: Ws2812TxChannel,
{
    pub fn new(channel: Tx) -> Self {
        Self {
            channel,
            buf: [Rgb { r: 0, g: 0, b: 0 }; pins::LED_COUNT],
        }
    }

    /// One render tick: resolve every slot's `Effect` at time `t_ms` and
    /// push the resulting colors out over RMT.
    pub fn render(&mut self, frame: &LedFrame, t_ms: u32) {
        for (slot_idx, slot) in frame.slots.iter().enumerate() {
            let idx = slot_to_chain_index(slot_idx);
            if idx < self.buf.len() {
                self.buf[idx] = resolve(slot, t_ms);
            }
        }
        // Underglow / remaining non-agent keys: left dark until the pinout
        // and a richer per-key LedFrame are available.
        // TODO(pinout): extend LedFrame (or add a second frame type) once
        // underglow / per-mechanical-key control is wired up.

        self.channel.write_ws2812(&self.buf);
    }
}

/// Narrow seam around the actual `esp_hal::rmt` TX API so `LedStrip` doesn't
/// need to name esp-hal's real (pin/peripheral-parameterized) channel type
/// in this skeleton. The real implementation encodes each `Rgb` as 24 RMT
/// items (GRB bit order, matching WS2812) followed by a >=50us reset gap,
/// using the timings in `ws2812_timing`.
pub trait Ws2812TxChannel {
    fn write_ws2812(&mut self, pixels: &[Rgb]);
}

// TODO: real impl once esp-hal's RMT channel type for LED_DATA_GPIO is
// known, roughly:
//
// impl Ws2812TxChannel for esp_hal::rmt::Channel<'_, Blocking, 0> {
//     fn write_ws2812(&mut self, pixels: &[Rgb]) {
//         let mut items: heapless::Vec<PulseCode, { pins::LED_COUNT * 24 + 1 }> = ...;
//         for px in pixels {
//             // WS2812 wire order is G, R, B (not R, G, B).
//             for byte in [px.g, px.r, px.b] {
//                 for bit in (0..8).rev() {
//                     let one = (byte >> bit) & 1 == 1;
//                     items.push(if one {
//                         PulseCode::new(Level::High, T1H_TICKS, Level::Low, T1L_TICKS)
//                     } else {
//                         PulseCode::new(Level::High, T0H_TICKS, Level::Low, T0L_TICKS)
//                     });
//                 }
//             }
//         }
//         items.push(PulseCode::new(Level::Low, RESET_LOW_TICKS, Level::Low, 0));
//         self.transmit(&items).wait().unwrap();
//     }
// }
