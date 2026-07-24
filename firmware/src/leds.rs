//! WS2812 LED chains, driven over SPI.
//!
//! **Not RMT.** The vendor firmware drives both chains from SPI hosts using
//! ESP-IDF's `led_strip` SPI backend, which the disassembly of their image
//! makes plain (see `docs/hardware/creator-micro-2-pinout-findings.md`). This
//! firmware does the same, because SPI is what the board is wired for: each
//! chain sits on its own host, so the two are independent buses and cannot be
//! daisy-chained.
//!
//! There are two chains:
//!
//! | Chain    | GPIO | LEDs | Purpose               |
//! |----------|------|------|-----------------------|
//! | Per-key  | 7    | 13   | one per mechanical key |
//! | Underglow| 6    | 8    | underglow              |
//!
//! The bit-level encoding lives in `openmicro_effects::ws2812`, on the host
//! side of the fence, so it is unit-tested rather than merely asserted. This
//! module is the thin part: keep a pixel buffer, resolve effects into it, hand
//! the encoded bytes to SPI.
//!
//! Not yet exercised on hardware.

use openmicro_effects::ws2812;
use openmicro_effects::{resolve, Rgb};
use openmicro_proto::LedFrame;

use crate::pins;

/// Render tick period. ~60 Hz is smooth for Breath/Pulse/Rainbow without
/// saturating the SPI bus or the BLE link.
pub const RENDER_PERIOD_MS: u64 = 16;

/// Encoded-byte buffer size for the per-key chain.
pub const PER_KEY_BUF_LEN: usize = ws2812::buffer_len(pins::PER_KEY_LED_COUNT);

/// Encoded-byte buffer size for the underglow chain.
pub const UNDERGLOW_BUF_LEN: usize = ws2812::buffer_len(pins::UNDERGLOW_LED_COUNT);

/// Anything that can push a prepared byte buffer out of an SPI host.
///
/// Abstracted so the render logic does not name esp-hal's SPI type, which is
/// parameterised by peripheral instance and DMA channel. `main.rs` supplies
/// the real implementation for each of the two hosts.
pub trait PixelOut {
    /// Push a whole frame. Errors are dropped by callers: a lost frame is
    /// replaced by the next one 16 ms later, and there is nothing useful to do
    /// about it on a device with no display.
    fn write_pixels(&mut self, pixels: &[Rgb]) -> Result<(), ()>;
}

/// Which physical LED in the per-key chain shows a given agent slot.
///
/// The protocol has `SLOT_COUNT` agent slots and the board has 13 per-key
/// LEDs. Which physical key each slot should light is a product decision that
/// depends on the key-ID map, and that map is still unknown — so this is the
/// identity mapping for now, and slots beyond the chain are dropped rather
/// than wrapping onto an arbitrary key.
fn slot_to_chain_index(slot: usize) -> usize {
    slot
}

/// The per-key chain: agent state, one LED per key.
pub struct PerKeyChain<S> {
    out: S,
    pixels: [Rgb; pins::PER_KEY_LED_COUNT],
}

impl<S: PixelOut> PerKeyChain<S> {
    pub fn new(out: S) -> Self {
        Self {
            out,
            pixels: [Rgb { r: 0, g: 0, b: 0 }; pins::PER_KEY_LED_COUNT],
        }
    }

    /// One render tick: resolve every slot's effect at `t_ms` and push it out.
    pub fn render(&mut self, frame: &LedFrame, t_ms: u32) {
        for (slot_idx, slot) in frame.slots.iter().enumerate() {
            let idx = slot_to_chain_index(slot_idx);
            if idx < self.pixels.len() {
                self.pixels[idx] = resolve(slot, t_ms);
            }
        }
        self.flush();
    }

    /// Encode the current pixels and write them.
    fn flush(&mut self) {
        let _ = self.out.write_pixels(&self.pixels);
    }

    /// Draw one frame of the boot animation.
    pub fn render_startup(&mut self, t_ms: u32) {
        openmicro_effects::startup::frame(t_ms, &mut self.pixels);
        self.flush();
    }

    /// Paint every key the same colour. Bring-up aid.
    pub fn set_all(&mut self, colour: Rgb) {
        self.pixels = [colour; pins::PER_KEY_LED_COUNT];
        self.flush();
    }

    /// Turn every per-key LED off (idle sleep).
    pub fn blank(&mut self) {
        self.pixels = [Rgb { r: 0, g: 0, b: 0 }; pins::PER_KEY_LED_COUNT];
        self.flush();
    }
}

/// The underglow chain: one solid colour across all 8 LEDs.
///
/// Separate type rather than a generic over length, because it is a different
/// SPI host and a different concept — it shows device state, not agent state.
pub struct UnderglowChain<S> {
    out: S,
    pixels: [Rgb; pins::UNDERGLOW_LED_COUNT],
}

impl<S: PixelOut> UnderglowChain<S> {
    pub fn new(out: S) -> Self {
        Self {
            out,
            pixels: [Rgb { r: 0, g: 0, b: 0 }; pins::UNDERGLOW_LED_COUNT],
        }
    }

    /// Paint the whole chain one colour.
    pub fn set(&mut self, colour: Rgb) {
        self.pixels = [colour; pins::UNDERGLOW_LED_COUNT];
        let _ = self.out.write_pixels(&self.pixels);
    }

    pub fn blank(&mut self) {
        self.set(Rgb { r: 0, g: 0, b: 0 });
    }

    /// Draw one frame of the boot animation.
    pub fn render_startup(&mut self, t_ms: u32) {
        openmicro_effects::startup::frame(t_ms, &mut self.pixels);
        let _ = self.out.write_pixels(&self.pixels);
    }

    /// A slow breath, shown once the boot animation is over.
    ///
    /// Proof of life: without it a working board with no agents connected is
    /// indistinguishable from a dead one, which is exactly the ambiguity that
    /// makes bring-up painful.
    pub fn render_idle(&mut self, t_ms: u32) {
        const IDLE: openmicro_proto::LedSlot = openmicro_proto::LedSlot {
            color: Rgb { r: 40, g: 90, b: 255 },
            effect: openmicro_proto::Effect::Breath,
            brightness: 60,
        };
        self.set(resolve(&IDLE, t_ms));
    }
}
