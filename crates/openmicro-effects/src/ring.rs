//! Animations for the 8-LED underglow ring.
//!
//! The ring is the device's status line. Where the per-key LEDs answer "which
//! agent, and what is it doing", the ring answers the one question you want
//! from across the desk: *does this need me?*
//!
//! Every motion has a distinct **shape**, not merely a distinct speed:
//!
//! | Motion      | Shape                          | Means                        |
//! |-------------|--------------------------------|------------------------------|
//! | `Breath`    | whole ring swells together     | calm; nothing is running     |
//! | `Spin`      | a comet travels round          | something is running         |
//! | `Alert`     | sharp double-blink             | a decision is waiting on you |
//! | `Aurora`    | slow cool multi-hue drift      | no host at all               |
//! | `Searching` | one dim dot orbiting slowly    | host up, daemon down         |
//!
//! Shape rather than speed because two of these are never on screen at the
//! same time, so there is nothing to compare a speed against. A swell and a
//! travelling dot are told apart in one glance with no reference.
//!
//! All of it is a pure function of `(t_ms, index)`, so it renders identically
//! on the host and on the device and can be tested without either.

use openmicro_proto::{Glow, Motion, Rgb, NOMINAL_SPEED};

use crate::{hsv_to_rgb, scale, triangle};

/// One full revolution of the `Spin` comet at nominal speed.
const SPIN_PERIOD_MS: u32 = 1200;
/// How far the comet's tail trails behind its head, in LEDs.
const SPIN_TAIL_LEDS: u32 = 3;

/// One `Breath` swell at nominal speed.
const BREATH_PERIOD_MS: u32 = 3400;
/// Floor of the breath, as a percent of full — it dips, it does not blink.
const BREATH_MIN_PCT: u32 = 12;

/// The `Alert` cycle: two quick flashes, then a gap.
const ALERT_PERIOD_MS: u32 = 1100;
const ALERT_FLASH_MS: u32 = 90;
const ALERT_GAP_MS: u32 = 110;

/// One revolution of the `Searching` dot — deliberately slow and patient.
const SEARCH_PERIOD_MS: u32 = 4200;
const SEARCH_TAIL_LEDS: u32 = 1;
/// The dot is a background hint, not a notification.
const SEARCH_MAX_PCT: u32 = 45;

/// One `Aurora` drift.
const AURORA_PERIOD_MS: u32 = 9000;
/// Aurora stays in the cool band: cyan through to blue. Warm hues are reserved
/// for things that want attention, and "no host" does not.
///
/// The upper bound stops at 168 rather than running further into the blues
/// because the hue wheel turns violet at 172 — and violet reads as *warm*
/// again, with red climbing back above green. A test asserts the band never
/// crosses that line.
const AURORA_HUE_MIN: u32 = 112;
const AURORA_HUE_MAX: u32 = 168;
/// Aurora never goes fully dark — it is ambient, so it keeps a floor.
const AURORA_MIN_PCT: u32 = 35;

/// Sub-LED resolution for the travelling motions.
///
/// Positions are tracked in 1/256ths of an LED so a comet on an 8-LED ring
/// glides instead of stepping between eight discrete places, which at these
/// speeds is very visible.
const SUB: u32 = 256;

/// Scale a nominal period by a [`Glow::speed`], where [`NOMINAL_SPEED`] leaves
/// it unchanged and 255 roughly halves it.
///
/// Saturating at 1 ms keeps the callers' modulo arithmetic safe no matter what
/// arrives over the wire.
fn period_for(nominal_ms: u32, speed: u8) -> u32 {
    let speed = speed.max(1) as u32;
    ((nominal_ms * NOMINAL_SPEED as u32) / speed).max(1)
}

/// Brightness of a comet LED: 255 at the head, fading to 0 at the tail's end.
///
/// `head` and the returned position are in [`SUB`]ths of an LED.
fn comet_weight(index: usize, count: usize, head: u32, tail_leds: u32) -> u8 {
    let span = count as u32 * SUB;
    let at = index as u32 * SUB;
    // How far this LED sits *behind* the head, going the way the comet travels.
    let behind = (head + span - at) % span;
    let tail = tail_leds * SUB;
    if behind >= tail {
        return 0;
    }
    (255 - (behind * 255) / tail) as u8
}

/// Where the head of a travelling motion is at `t_ms`, in [`SUB`]ths of an LED.
fn head_at(t_ms: u32, period_ms: u32, count: usize) -> u32 {
    let span = count as u32 * SUB;
    ((t_ms % period_ms) as u64 * span as u64 / period_ms as u64) as u32
}

/// Percent-of-full brightness for the `Alert` double-blink at `t_ms`.
fn alert_level(t_ms: u32, period_ms: u32) -> u8 {
    let phase = t_ms % period_ms;
    let second_flash = ALERT_FLASH_MS + ALERT_GAP_MS;
    if phase < ALERT_FLASH_MS || (phase >= second_flash && phase < second_flash + ALERT_FLASH_MS) {
        255
    } else {
        0
    }
}

/// Render the whole ring for `glow` at `t_ms` into `out`.
///
/// `out` may be any length; the motions scale to it, so an 8-LED ring and a
/// hypothetical 12-LED one both look right. Nothing is allocated.
pub fn frame(glow: &Glow, t_ms: u32, out: &mut [Rgb]) {
    let count = out.len();
    if count == 0 {
        return;
    }
    if glow.brightness == 0 || glow.motion == Motion::Off {
        for px in out.iter_mut() {
            *px = Rgb { r: 0, g: 0, b: 0 };
        }
        return;
    }

    match glow.motion {
        Motion::Off => unreachable!("handled above"),

        Motion::Breath => {
            let period = period_for(BREATH_PERIOD_MS, glow.speed);
            let level = envelope(glow.brightness, triangle(t_ms, period), BREATH_MIN_PCT);
            for px in out.iter_mut() {
                *px = scale(glow.color, level);
            }
        }

        Motion::Spin => {
            let period = period_for(SPIN_PERIOD_MS, glow.speed);
            let head = head_at(t_ms, period, count);
            for (i, px) in out.iter_mut().enumerate() {
                let w = comet_weight(i, count, head, SPIN_TAIL_LEDS);
                *px = scale(glow.color, mul(glow.brightness, w));
            }
        }

        Motion::Alert => {
            let period = period_for(ALERT_PERIOD_MS, glow.speed);
            let level = mul(glow.brightness, alert_level(t_ms, period));
            for px in out.iter_mut() {
                *px = scale(glow.color, level);
            }
        }

        Motion::Aurora => {
            // Ignores `glow.color` on purpose: this is the state where there is
            // no host, so there is no agent whose colour it could be.
            let period = period_for(AURORA_PERIOD_MS, glow.speed);
            let span = AURORA_HUE_MAX - AURORA_HUE_MIN;
            for (i, px) in out.iter_mut().enumerate() {
                // Offsetting each LED's phase by a fraction of the period is
                // what makes it drift around the ring rather than pulse as one.
                let phase = t_ms.wrapping_add((i as u32 * period) / count as u32);
                let hue = AURORA_HUE_MIN + (triangle(phase, period) as u32 * span) / 255;
                // A second, faster wave keeps it shimmering while it drifts.
                let shimmer = triangle(phase.wrapping_mul(2), period);
                let level = envelope(glow.brightness, shimmer, AURORA_MIN_PCT);
                *px = hsv_to_rgb(hue as u8, 255, level);
            }
        }

        Motion::Searching => {
            let period = period_for(SEARCH_PERIOD_MS, glow.speed);
            let head = head_at(t_ms, period, count);
            let ceiling = ((glow.brightness as u32 * SEARCH_MAX_PCT) / 100) as u8;
            for (i, px) in out.iter_mut().enumerate() {
                let w = comet_weight(i, count, head, SEARCH_TAIL_LEDS);
                *px = scale(glow.color, mul(ceiling, w));
            }
        }
    }
}

/// `a * b / 255`, for composing two 0..=255 brightness factors.
fn mul(a: u8, b: u8) -> u8 {
    ((a as u16 * b as u16) / 255) as u8
}

/// Map a 0..=255 wave onto `min_pct%..100%` of `ceiling`.
fn envelope(ceiling: u8, wave: u8, min_pct: u32) -> u8 {
    let max = ceiling as u32;
    let min = (max * min_pct) / 100;
    (min + ((max - min) * wave as u32) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    const COUNT: usize = openmicro_proto::layout::UNDERGLOW_COUNT;

    fn glow(motion: Motion, brightness: u8) -> Glow {
        Glow {
            color: Rgb { r: 200, g: 100, b: 50 },
            motion,
            brightness,
            speed: NOMINAL_SPEED,
        }
    }

    fn render(g: &Glow, t_ms: u32) -> std::vec::Vec<Rgb> {
        let mut out = vec![Rgb { r: 0, g: 0, b: 0 }; COUNT];
        frame(g, t_ms, &mut out);
        out
    }

    fn total(px: &[Rgb]) -> u32 {
        px.iter().map(|p| p.r as u32 + p.g as u32 + p.b as u32).sum()
    }

    fn lit_count(px: &[Rgb]) -> usize {
        px.iter().filter(|p| p.r > 0 || p.g > 0 || p.b > 0).count()
    }

    #[test]
    fn off_and_zero_brightness_are_both_dark() {
        assert_eq!(total(&render(&glow(Motion::Off, 255), 500)), 0);
        // Brightness 0 must win even for a motion that would otherwise light
        // up, or "sleep" would not actually turn the ring off.
        for motion in [Motion::Breath, Motion::Spin, Motion::Alert, Motion::Aurora, Motion::Searching] {
            assert_eq!(total(&render(&glow(motion, 0), 500)), 0, "{motion:?} at brightness 0");
        }
    }

    #[test]
    fn an_empty_ring_is_not_a_panic() {
        let mut none: [Rgb; 0] = [];
        frame(&glow(Motion::Spin, 255), 100, &mut none);
    }

    #[test]
    fn breath_lights_the_whole_ring_evenly() {
        let px = render(&glow(Motion::Breath, 255), BREATH_PERIOD_MS / 2);
        assert_eq!(lit_count(&px), COUNT, "breath is the whole ring, not a part of it");
        for p in &px {
            assert_eq!(*p, px[0], "every LED shares one level");
        }
    }

    #[test]
    fn breath_dips_but_never_goes_dark() {
        let g = glow(Motion::Breath, 255);
        let trough = render(&g, 0);
        let peak = render(&g, BREATH_PERIOD_MS / 2);
        assert!(total(&peak) > total(&trough), "the swell must actually swell");
        // A breath that reaches zero reads as a blink, which is Alert's job.
        assert!(total(&trough) > 0, "breath bottomed out at zero");
    }

    #[test]
    fn spin_is_a_comet_not_the_whole_ring() {
        let px = render(&glow(Motion::Spin, 255), 0);
        let lit = lit_count(&px);
        assert!(lit > 0, "the comet has to be somewhere");
        assert!(lit < COUNT, "a comet that lights everything is just Breath");
        assert!(lit <= SPIN_TAIL_LEDS as usize, "tail longer than configured: {lit}");
    }

    #[test]
    fn spin_travels_all_the_way_round_and_returns() {
        let g = glow(Motion::Spin, 255);
        let start = render(&g, 0);
        // Every LED must be the brightest one at some point in a revolution,
        // or part of the ring is dead.
        let mut was_head = [false; COUNT];
        for step in 0..SPIN_PERIOD_MS {
            let px = render(&g, step);
            let (best, _) = px
                .iter()
                .enumerate()
                .max_by_key(|(_, p)| p.r as u32 + p.g as u32 + p.b as u32)
                .unwrap();
            was_head[best] = true;
        }
        assert!(was_head.iter().all(|x| *x), "comet never reached every LED: {was_head:?}");
        assert_eq!(render(&g, SPIN_PERIOD_MS), start, "one period returns to the start");
    }

    #[test]
    fn spin_head_is_brighter_than_its_tail() {
        let px = render(&glow(Motion::Spin, 255), 0);
        // At t=0 the head sits on LED 0 and the tail trails backwards round the
        // ring, so the last LEDs are the dim end.
        let head = px[0].r as u32;
        let behind = px[COUNT - 1].r as u32;
        assert!(head > behind, "head {head} should lead tail {behind}");
    }

    #[test]
    fn alert_is_two_sharp_flashes_then_a_gap() {
        let g = glow(Motion::Alert, 255);
        // Sampled across one cycle, it must be fully on twice and fully off in
        // between — a double-tap, not a fade.
        assert!(total(&render(&g, 0)) > 0, "first flash");
        assert_eq!(total(&render(&g, ALERT_FLASH_MS + 10)), 0, "gap between flashes");
        assert!(total(&render(&g, ALERT_FLASH_MS + ALERT_GAP_MS + 10)) > 0, "second flash");
        assert_eq!(total(&render(&g, ALERT_PERIOD_MS - 10)), 0, "long dark rest");
    }

    #[test]
    fn alert_spends_most_of_its_cycle_dark() {
        // The rest between double-taps is what makes it read as urgent rather
        // than as a strobe. Verify the duty cycle is genuinely low.
        let g = glow(Motion::Alert, 255);
        let lit = (0..ALERT_PERIOD_MS).filter(|t| total(&render(&g, *t)) > 0).count();
        let duty = lit * 100 / ALERT_PERIOD_MS as usize;
        assert!(duty < 30, "alert duty cycle {duty}% is closer to a strobe than a blink");
    }

    #[test]
    fn alert_lights_the_ring_as_one() {
        let px = render(&glow(Motion::Alert, 255), 0);
        assert_eq!(lit_count(&px), COUNT);
        for p in &px {
            assert_eq!(*p, px[0]);
        }
    }

    #[test]
    fn aurora_is_cool_hued_and_ignores_the_slot_color() {
        // It runs when there is no host, so there is no agent colour to honour.
        let mut warm = glow(Motion::Aurora, 255);
        warm.color = Rgb { r: 255, g: 0, b: 0 };
        let mut cool = glow(Motion::Aurora, 255);
        cool.color = Rgb { r: 0, g: 255, b: 0 };
        assert_eq!(render(&warm, 1234), render(&cool, 1234), "aurora must ignore colour");

        // And it must actually look cool: blue/green dominant, never red.
        for t in [0, 900, 3000, 5500, 8999] {
            for p in render(&warm, t) {
                assert!(
                    p.b >= p.r && p.g >= p.r,
                    "aurora went warm at t={t}: {p:?}"
                );
            }
        }
    }

    #[test]
    fn aurora_varies_around_the_ring_rather_than_pulsing_as_one() {
        // The per-LED phase offset is the whole reason it reads as a drift.
        let px = render(&glow(Motion::Aurora, 255), 2000);
        assert!(px.iter().any(|p| *p != px[0]), "aurora is uniform, so it is not drifting");
    }

    #[test]
    fn aurora_keeps_a_floor_so_it_never_looks_switched_off() {
        let g = glow(Motion::Aurora, 255);
        for t in [0, 500, 2250, 4500, 7000, 8999] {
            assert_eq!(lit_count(&render(&g, t)), COUNT, "aurora went dark at t={t}");
        }
    }

    #[test]
    fn searching_is_a_single_dim_dot() {
        let g = glow(Motion::Searching, 255);
        let px = render(&g, 0);
        assert_eq!(lit_count(&px), 1, "searching should be one dot");

        // And dimmer than a notification would be, so it stays a background
        // hint rather than demanding attention.
        let alert = render(&glow(Motion::Alert, 255), 0);
        assert!(
            total(&px) * 2 < total(&alert),
            "searching dot is too loud: {} vs alert {}",
            total(&px),
            total(&alert)
        );
    }

    #[test]
    fn searching_is_slower_than_spin() {
        // The two are both travelling motions, so speed is what separates
        // "working on it" from "looking for the daemon".
        assert!(SEARCH_PERIOD_MS > SPIN_PERIOD_MS * 2);
    }

    #[test]
    fn speed_scales_a_motion_without_changing_its_shape() {
        let mut fast = glow(Motion::Spin, 255);
        fast.speed = 255;
        let slow = glow(Motion::Spin, 255);
        // At double speed the comet is twice as far round at the same instant.
        let half = SPIN_PERIOD_MS / 4;
        let fast_px = render(&fast, half);
        let slow_px = render(&slow, half);
        assert_ne!(fast_px, slow_px, "speed had no effect");
        // Shape is preserved: still a comet, still the same tail length.
        assert!(lit_count(&fast_px) <= SPIN_TAIL_LEDS as usize);
    }

    #[test]
    fn a_zero_speed_does_not_divide_by_zero() {
        let mut g = glow(Motion::Spin, 255);
        g.speed = 0;
        let _ = render(&g, 999); // must not panic
        for motion in [Motion::Breath, Motion::Alert, Motion::Aurora, Motion::Searching] {
            g.motion = motion;
            let _ = render(&g, 999);
        }
    }

    #[test]
    fn brightness_is_a_ceiling_for_every_motion() {
        for motion in [Motion::Breath, Motion::Spin, Motion::Alert, Motion::Searching] {
            let dim = glow(motion, 40);
            let bright = glow(motion, 255);
            // Sample across a cycle: the dim version must never out-shine the
            // bright one at the same instant.
            for t in (0..SEARCH_PERIOD_MS).step_by(37) {
                assert!(
                    total(&render(&dim, t)) <= total(&render(&bright, t)),
                    "{motion:?} exceeded its ceiling at t={t}"
                );
            }
        }
    }

    #[test]
    fn comet_weight_wraps_around_the_ring_seam() {
        // A comet whose head has just passed LED 0 must still light LED 7.
        // Getting the modulo wrong here shows up as the tail vanishing once per
        // revolution, which is easy to miss and awful to look at.
        let head = 0;
        assert_eq!(comet_weight(0, COUNT, head, 3), 255, "head is full brightness");
        assert!(comet_weight(COUNT - 1, COUNT, head, 3) > 0, "tail must cross the seam");
        assert_eq!(comet_weight(COUNT / 2, COUNT, head, 3), 0, "far side stays dark");
    }

    #[test]
    fn period_for_treats_nominal_speed_as_unchanged() {
        assert_eq!(period_for(1000, NOMINAL_SPEED), 1000);
        assert!(period_for(1000, 255) < 1000, "higher speed means a shorter period");
        assert!(period_for(1000, 64) > 1000, "lower speed means a longer period");
        assert!(period_for(1, 255) >= 1, "period never reaches zero");
    }
}
