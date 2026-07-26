use openmicro_effects::status::{self, Link};
use openmicro_effects::{resolve, ring, Rgb};
use openmicro_proto::layout;
use openmicro_proto::{Glow, LedFrame};

use crate::pins;

pub const RENDER_PERIOD_MS: u64 = 16;

pub const PER_KEY_BUF_LEN: usize = openmicro_effects::ws2812::buffer_len(pins::PER_KEY_LED_COUNT);

pub const UNDERGLOW_BUF_LEN: usize =
    openmicro_effects::ws2812::buffer_len(pins::UNDERGLOW_LED_COUNT);

pub trait PixelOut {
    fn write_pixels(&mut self, pixels: &[Rgb]) -> Result<(), ()>;
}

pub struct KeyChain<S> {
    out: S,
    pixels: [Rgb; pins::PER_KEY_LED_COUNT],
}

const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

impl<S: PixelOut> KeyChain<S> {
    pub fn new(out: S) -> Self {
        Self { out, pixels: [BLACK; pins::PER_KEY_LED_COUNT] }
    }

    fn set_key(&mut self, key: u8, colour: Rgb) {
        let Some(led) = layout::LED_FOR_KEY.get(key as usize) else { return };
        if let Some(px) = self.pixels.get_mut(*led as usize) {
            *px = colour;
        }
    }

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

    pub fn render_startup(&mut self, t_ms: u32) {
        openmicro_effects::startup::frame(t_ms, &mut self.pixels);
        self.flush();
    }

    pub fn set_chain_index(&mut self, index: usize, colour: Rgb) {
        self.pixels = [BLACK; pins::PER_KEY_LED_COUNT];
        if let Some(px) = self.pixels.get_mut(index) {
            *px = colour;
        }
        self.flush();
    }

    pub fn set_chain_indices(&mut self, lit: &[(usize, Rgb)]) {
        self.pixels = [BLACK; pins::PER_KEY_LED_COUNT];
        for (index, colour) in lit {
            if let Some(px) = self.pixels.get_mut(*index) {
                *px = *colour;
            }
        }
        self.flush();
    }

    pub fn blank(&mut self) {
        self.pixels = [BLACK; pins::PER_KEY_LED_COUNT];
        self.flush();
    }
}

pub struct GlowRing<S> {
    out: S,
    pixels: [Rgb; pins::UNDERGLOW_LED_COUNT],
}

impl<S: PixelOut> GlowRing<S> {
    pub fn new(out: S) -> Self {
        Self { out, pixels: [BLACK; pins::UNDERGLOW_LED_COUNT] }
    }

    pub fn render(&mut self, glow: &Glow, t_ms: u32) {
        ring::frame(glow, t_ms, &mut self.pixels);
        let _ = self.out.write_pixels(&self.pixels);
    }

    pub fn render_link(&mut self, link: Link, brightness: u8, t_ms: u32) {
        self.render(&status::local_glow(link, brightness), t_ms);
    }

    pub fn render_startup(&mut self, t_ms: u32) {
        openmicro_effects::startup::frame(t_ms, &mut self.pixels);
        let _ = self.out.write_pixels(&self.pixels);
    }

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
