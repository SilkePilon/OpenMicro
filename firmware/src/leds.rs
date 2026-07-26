//! The two WS2812 chains.
//!
//! | Chain     | GPIO | LEDs | Shows                                    |
//! |-----------|------|------|------------------------------------------|
//! | Per-key   | 7    | 13   | which agent, and what it is doing        |
//! | Underglow | 6    | 8    | overall status, readable across the room |
//!
//! Both live on the **upper PCB**, which is power-gated behind
//! `pins::TOP_BOARD_POWER`. Nothing here works until those pins are driven, and
//! the failure mode is silent: every write succeeds and nothing lights. That
//! cost a night once — see `docs/hardware/creator-micro-2-pinout-findings.md`.
//!
//! # What this module does and does not decide
//!
//! It decides *nothing* about colour or motion. Those all come from
//! `openmicro_effects::status` and `::ring`, which the daemon also uses, so the
//! two ends cannot drift. This module only owns pixel buffers, the key-to-chain
//! remap, and the fallbacks for when the host has gone quiet.
//!
//! The bit-level encoding lives in `openmicro_effects::ws2812` (SPI) or is done
//! by RMT in `main.rs`; either way it is host-tested rather than merely
//! asserted.

use openmicro_effects::status::{self, Link};
use openmicro_effects::{resolve, ring, Rgb};
use openmicro_proto::layout;
use openmicro_proto::{Glow, LedFrame};

use crate::pins;

/// Render tick period. ~60 Hz, which the travelling motions need to glide
/// rather than step; slower and `Motion::Spin` visibly stutters.
pub const RENDER_PERIOD_MS: u64 = 16;

/// Encoded-byte buffer size for the per-key chain.
pub const PER_KEY_BUF_LEN: usize = openmicro_effects::ws2812::buffer_len(pins::PER_KEY_LED_COUNT);

/// Encoded-byte buffer size for the underglow chain.
pub const UNDERGLOW_BUF_LEN: usize =
    openmicro_effects::ws2812::buffer_len(pins::UNDERGLOW_LED_COUNT);

/// Anything that can push a prepared frame of pixels out of a peripheral.
///
/// Abstracted so the render logic never names esp-hal's RMT or SPI types, which
/// are parameterised by peripheral instance and DMA channel. `main.rs` supplies
/// the real implementation for each chain.
pub trait PixelOut {
    /// Push a whole frame. Errors are dropped by callers: a lost frame is
    /// replaced by the next one 16 ms later, and there is nothing useful to do
    /// about it on a device with no display.
    fn write_pixels(&mut self, pixels: &[Rgb]) -> Result<(), ()>;
}

/// The per-key chain.
pub struct KeyChain<S> {
    out: S,
    pixels: [Rgb; pins::PER_KEY_LED_COUNT],
}

const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

impl<S: PixelOut> KeyChain<S> {
    pub fn new(out: S) -> Self {
        Self { out, pixels: [BLACK; pins::PER_KEY_LED_COUNT] }
    }

    /// Paint one key, by key id.
    ///
    /// This is the only place the key-to-chain remap is applied, so a wrong
    /// `LED_FOR_KEY` shows up as the whole board being permuted rather than as
    /// one subsystem disagreeing with another.
    fn set_key(&mut self, key: u8, colour: Rgb) {
        let Some(led) = layout::LED_FOR_KEY.get(key as usize) else { return };
        if let Some(px) = self.pixels.get_mut(*led as usize) {
            *px = colour;
        }
    }

    /// One render tick from a host frame: resolve every key's effect at `t_ms`.
    pub fn render(&mut self, frame: &LedFrame, t_ms: u32) {
        let slots = status::key_slots(frame);
        for (key, slot) in slots.iter().enumerate() {
            self.set_key(key as u8, resolve(slot, t_ms));
        }
        self.flush();
    }

    fn flush(&mut self) {
        let _ = self.out.write_pixels(&self.pixels);
    }

    /// Draw one frame of the boot animation.
    pub fn render_startup(&mut self, t_ms: u32) {
        openmicro_effects::startup::frame(t_ms, &mut self.pixels);
        self.flush();
    }

    /// Light exactly one chain position, by *chain index* rather than key id.
    ///
    /// The bring-up aid for `LED_FOR_KEY`: step through the chain, note which
    /// key each index appears under, and the table writes itself. Takes a raw
    /// index on purpose — going through the remap would assume the very thing
    /// being measured.
    pub fn set_chain_index(&mut self, index: usize, colour: Rgb) {
        self.pixels = [BLACK; pins::PER_KEY_LED_COUNT];
        if let Some(px) = self.pixels.get_mut(index) {
            *px = colour;
        }
        self.flush();
    }

    pub fn blank(&mut self) {
        self.pixels = [BLACK; pins::PER_KEY_LED_COUNT];
        self.flush();
    }
}

/// The underglow ring.
///
/// A separate type from [`KeyChain`], not a generic over length, because it is a
/// different peripheral *and* a different concept: it shows device status, not
/// agent state, and it is the one thing that keeps working when the host does
/// not.
pub struct GlowRing<S> {
    out: S,
    pixels: [Rgb; pins::UNDERGLOW_LED_COUNT],
}

impl<S: PixelOut> GlowRing<S> {
    pub fn new(out: S) -> Self {
        Self { out, pixels: [BLACK; pins::UNDERGLOW_LED_COUNT] }
    }

    /// Draw one frame of a [`Glow`].
    pub fn render(&mut self, glow: &Glow, t_ms: u32) {
        ring::frame(glow, t_ms, &mut self.pixels);
        let _ = self.out.write_pixels(&self.pixels);
    }

    /// Draw the ring for a link state the device worked out for itself.
    ///
    /// This is the whole reason the firmware carries a copy of the design: when
    /// the daemon is the thing that is missing, it cannot be the one to say so.
    pub fn render_link(&mut self, link: Link, brightness: u8, t_ms: u32) {
        self.render(&status::local_glow(link, brightness), t_ms);
    }

    /// Draw one frame of the boot animation.
    pub fn render_startup(&mut self, t_ms: u32) {
        openmicro_effects::startup::frame(t_ms, &mut self.pixels);
        let _ = self.out.write_pixels(&self.pixels);
    }

    /// Light exactly one ring position. Bring-up aid, as for the key chain.
    pub fn set_chain_index(&mut self, index: usize, colour: Rgb) {
        self.pixels = [BLACK; pins::UNDERGLOW_LED_COUNT];
        if let Some(px) = self.pixels.get_mut(index) {
            *px = colour;
        }
        let _ = self.out.write_pixels(&self.pixels);
    }

    pub fn blank(&mut self) {
        self.pixels = [BLACK; pins::UNDERGLOW_LED_COUNT];
        let _ = self.out.write_pixels(&self.pixels);
    }
}

// The link-state decision itself is `openmicro_effects::status::link_state`, on
// the host side of the fence so it is unit-tested rather than merely asserted.
