#![no_std]

#[cfg(test)]
extern crate std;

pub mod demo;
pub mod power;
pub mod ring;
pub mod startup;
pub mod status;
pub mod ws2812;

pub use openmicro_proto::{Effect, LedSlot, Rgb};

const BREATH_PERIOD_MS: u32 = 3000;
const BREATH_MIN_PCT: u32 = 15;

const PULSE_PERIOD_MS: u32 = 700;
const PULSE_MIN_PCT: u32 = 10;

const RAINBOW_PERIOD_MS: u32 = 4000;

pub fn resolve(slot: &LedSlot, t_ms: u32) -> Rgb {
    if slot.brightness == 0 {
        return Rgb { r: 0, g: 0, b: 0 };
    }
    match slot.effect {
        Effect::Solid => scale(slot.color, slot.brightness),
        Effect::Breath => {
            let b = lfo_brightness(slot.brightness, t_ms, BREATH_PERIOD_MS, BREATH_MIN_PCT, false);
            scale(slot.color, b)
        }
        Effect::Pulse => {
            let b = lfo_brightness(slot.brightness, t_ms, PULSE_PERIOD_MS, PULSE_MIN_PCT, true);
            scale(slot.color, b)
        }
        Effect::Rainbow => {
            let hue = rainbow_hue(t_ms);
            hsv_to_rgb(hue, 255, slot.brightness)
        }
    }
}

pub fn gamma_channel(v: u8) -> u8 {
    ((v as u16 * v as u16) / 255) as u8
}

pub fn gamma(c: Rgb) -> Rgb {
    Rgb { r: gamma_channel(c.r), g: gamma_channel(c.g), b: gamma_channel(c.b) }
}

pub fn scale(c: Rgb, brightness: u8) -> Rgb {
    Rgb {
        r: scale_channel(c.r, brightness),
        g: scale_channel(c.g, brightness),
        b: scale_channel(c.b, brightness),
    }
}

fn scale_channel(v: u8, brightness: u8) -> u8 {
    ((v as u16 * brightness as u16) / 255) as u8
}

pub fn hsv_to_rgb(h: u8, s: u8, v: u8) -> Rgb {
    if s == 0 {
        return Rgb { r: v, g: v, b: v };
    }
    let h = h as u32;
    let s = s as u32;
    let v = v as u32;

    let region = h / 43;
    let remainder = (h - region * 43) * 6;

    let p = (v * (255 - s)) / 255;
    let q = (v * (255 - (s * remainder) / 255)) / 255;
    let t = (v * (255 - (s * (255 - remainder)) / 255)) / 255;

    let (r, g, b) = match region {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Rgb { r: r as u8, g: g as u8, b: b as u8 }
}

pub fn triangle(t_ms: u32, period_ms: u32) -> u8 {
    let period_ms = period_ms.max(2);
    let phase = t_ms % period_ms;
    let half = period_ms / 2;
    let v = if phase <= half {
        (phase * 255) / half
    } else {
        ((period_ms - phase) * 255) / half
    };
    v.min(255) as u8
}

fn triangle_wave(t_ms: u32, period_ms: u32) -> u32 {
    triangle(t_ms, period_ms) as u32
}

fn lfo_brightness(max_brightness: u8, t_ms: u32, period_ms: u32, min_pct: u32, sharp: bool) -> u8 {
    let tri = triangle_wave(t_ms, period_ms);
    let shaped = if sharp { (tri * tri) / 255 } else { tri };

    let max_b = max_brightness as u32;
    let min_b = (max_b * min_pct) / 100;
    let range = max_b - min_b;
    (min_b + (range * shaped) / 255) as u8
}

fn rainbow_hue(t_ms: u32) -> u8 {
    let phase = t_ms % RAINBOW_PERIOD_MS;
    ((phase * 255) / RAINBOW_PERIOD_MS) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(color: Rgb, effect: Effect, brightness: u8) -> LedSlot {
        LedSlot { color, effect, brightness }
    }

    #[test]
    fn gamma_is_monotonic_and_keeps_the_endpoints() {
        assert_eq!(gamma_channel(0), 0);
        assert!(gamma_channel(255) > 235, "full scale must stay near full");
        for v in 1..=255u8 {
            assert!(
                gamma_channel(v) >= gamma_channel(v - 1),
                "gamma dips at {v}: {} then {}",
                gamma_channel(v - 1),
                gamma_channel(v)
            );
        }
    }

    #[test]
    fn gamma_pulls_the_midpoint_well_below_half() {
        let mid = gamma_channel(128);
        assert!(mid < 90, "gamma is too weak to fix a crossfade: {mid}");
        assert!(mid > 50, "gamma is crushing the mid-range: {mid}");
    }

    #[test]
    fn gamma_never_makes_a_step_bigger_than_the_input_step() {
        for v in 1..=255u8 {
            let step = gamma_channel(v) as i32 - gamma_channel(v - 1) as i32;
            assert!(step <= 3, "gamma jumps {step} at {v}");
        }
    }

    #[test]
    fn gamma_applies_to_every_channel() {
        let c = Rgb { r: 255, g: 128, b: 0 };
        let g = gamma(c);
        assert_eq!(g.r, gamma_channel(255));
        assert_eq!(g.g, gamma_channel(128));
        assert_eq!(g.b, 0);
    }

    #[test]
    fn scale_halves_full_white_at_128() {
        let c = Rgb { r: 255, g: 255, b: 255 };
        let got = scale(c, 128);
        assert_eq!(got, Rgb { r: 128, g: 128, b: 128 });
    }

    #[test]
    fn scale_zero_brightness_is_black() {
        let c = Rgb { r: 200, g: 150, b: 90 };
        assert_eq!(scale(c, 0), Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn scale_full_brightness_is_unchanged() {
        let c = Rgb { r: 200, g: 150, b: 90 };
        assert_eq!(scale(c, 255), c);
    }

    #[test]
    fn hsv_zero_saturation_is_gray() {
        assert_eq!(hsv_to_rgb(123, 0, 200), Rgb { r: 200, g: 200, b: 200 });
    }

    #[test]
    fn hsv_red_at_hue_zero_full_sat_val() {
        assert_eq!(hsv_to_rgb(0, 255, 255), Rgb { r: 255, g: 0, b: 0 });
    }

    #[test]
    fn hsv_green_near_hue_85() {
        let got = hsv_to_rgb(85, 255, 255);
        assert_eq!(got.g, 255);
        assert!(got.r <= 3, "expected r near 0, got {}", got.r);
        assert_eq!(got.b, 0);
    }

    #[test]
    fn resolve_solid_at_brightness_128_halves_each_channel() {
        let s = slot(Rgb { r: 255, g: 255, b: 255 }, Effect::Solid, 128);
        assert_eq!(resolve(&s, 0), Rgb { r: 128, g: 128, b: 128 });
        assert_eq!(resolve(&s, 999_999), Rgb { r: 128, g: 128, b: 128 });
    }

    #[test]
    fn resolve_off_slot_is_black_regardless_of_effect() {
        assert_eq!(resolve(&LedSlot::OFF, 0), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(resolve(&LedSlot::OFF, 1500), Rgb { r: 0, g: 0, b: 0 });

        let off_rainbow = slot(Rgb { r: 10, g: 20, b: 30 }, Effect::Rainbow, 0);
        assert_eq!(resolve(&off_rainbow, 500), Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn resolve_breath_trough_dimmer_than_peak() {
        let s = slot(Rgb { r: 255, g: 255, b: 255 }, Effect::Breath, 255);
        let trough = resolve(&s, 0);
        let peak = resolve(&s, BREATH_PERIOD_MS / 2);
        assert!(
            peak.r > trough.r,
            "expected peak ({}) brighter than trough ({})",
            peak.r,
            trough.r
        );
        assert_eq!(peak, Rgb { r: 255, g: 255, b: 255 });
        assert!(trough.r > 0);
        assert!(trough.r < peak.r);
    }

    #[test]
    fn resolve_pulse_trough_dimmer_than_peak() {
        let s = slot(Rgb { r: 255, g: 255, b: 255 }, Effect::Pulse, 255);
        let trough = resolve(&s, 0);
        let peak = resolve(&s, PULSE_PERIOD_MS / 2);
        assert!(peak.r > trough.r);
        assert_eq!(peak, Rgb { r: 255, g: 255, b: 255 });
    }

    #[test]
    fn resolve_pulse_is_sharper_than_breath_at_quarter_period() {
        let breath = slot(Rgb { r: 255, g: 255, b: 255 }, Effect::Breath, 255);
        let pulse = slot(Rgb { r: 255, g: 255, b: 255 }, Effect::Pulse, 255);
        let breath_mid = resolve(&breath, BREATH_PERIOD_MS / 4);
        let pulse_mid = resolve(&pulse, PULSE_PERIOD_MS / 4);
        assert!(pulse_mid.r < breath_mid.r);
    }

    #[test]
    fn resolve_rainbow_differs_at_t0_vs_t2000() {
        let s = slot(Rgb { r: 0, g: 0, b: 0 }, Effect::Rainbow, 255);
        let a = resolve(&s, 0);
        let b = resolve(&s, 2000);
        assert_ne!(a, b);
    }

    #[test]
    fn resolve_rainbow_ignores_slot_color() {
        let red = slot(Rgb { r: 255, g: 0, b: 0 }, Effect::Rainbow, 255);
        let blue = slot(Rgb { r: 0, g: 0, b: 255 }, Effect::Rainbow, 255);
        assert_eq!(resolve(&red, 500), resolve(&blue, 500));
    }

    #[test]
    fn resolve_rainbow_full_cycle_returns_to_start() {
        let s = slot(Rgb { r: 0, g: 0, b: 0 }, Effect::Rainbow, 255);
        assert_eq!(resolve(&s, 0), resolve(&s, RAINBOW_PERIOD_MS));
    }
}
