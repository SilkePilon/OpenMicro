use openmicro_proto::{Glow, Motion, Rgb, NOMINAL_SPEED};

use crate::{hsv_to_rgb, scale, triangle};

const SPIN_PERIOD_MS: u32 = 1200;
const SPIN_TAIL_LEDS: u32 = 3;

const BREATH_PERIOD_MS: u32 = 3400;
const BREATH_MIN_PCT: u32 = 25;

const ALERT_PERIOD_MS: u32 = 1100;
const ALERT_FLASH_MS: u32 = 90;
const ALERT_GAP_MS: u32 = 110;

const SEARCH_PERIOD_MS: u32 = 4200;
const SEARCH_TAIL_LEDS: u32 = 1;
const SEARCH_MAX_PCT: u32 = 70;

const AURORA_PERIOD_MS: u32 = 9000;
const AURORA_HUE_MIN: u32 = 112;
const AURORA_HUE_MAX: u32 = 168;
const AURORA_MIN_PCT: u32 = 50;

const SUB: u32 = 256;

fn period_for(nominal_ms: u32, speed: u8) -> u32 {
    let speed = speed.max(1) as u32;
    ((nominal_ms * NOMINAL_SPEED as u32) / speed).max(1)
}

const COMET_LEAD_LEDS: u32 = 1;

fn comet_weight(index: usize, count: usize, head: u32, tail_leds: u32) -> u8 {
    let span = count as u32 * SUB;
    let at = index as u32 * SUB;
    let behind = (head + span - at) % span;
    let tail = (tail_leds * SUB).max(1);
    let lead = (COMET_LEAD_LEDS * SUB).min(span / 2).max(1);

    if behind <= tail {
        (255 - (behind * 255) / tail) as u8
    } else {
        let ahead = span - behind;
        if ahead <= lead {
            (255 - (ahead * 255) / lead) as u8
        } else {
            0
        }
    }
}

fn head_at(t_ms: u32, period_ms: u32, count: usize) -> u32 {
    let span = count as u32 * SUB;
    ((t_ms % period_ms) as u64 * span as u64 / period_ms as u64) as u32
}

fn alert_level(t_ms: u32, period_ms: u32) -> u8 {
    let phase = t_ms % period_ms;
    let second_flash = ALERT_FLASH_MS + ALERT_GAP_MS;
    if phase < ALERT_FLASH_MS || (phase >= second_flash && phase < second_flash + ALERT_FLASH_MS) {
        255
    } else {
        0
    }
}

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
            let period = period_for(AURORA_PERIOD_MS, glow.speed);
            let span = AURORA_HUE_MAX - AURORA_HUE_MIN;
            for (i, px) in out.iter_mut().enumerate() {
                let phase = t_ms.wrapping_add((i as u32 * period) / count as u32);
                let hue = AURORA_HUE_MIN + (triangle(phase, period) as u32 * span) / 255;
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

fn mul(a: u8, b: u8) -> u8 {
    ((a as u16 * b as u16) / 255) as u8
}

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
        assert!(total(&trough) > 0, "breath bottomed out at zero");
    }

    #[test]
    fn spin_is_a_comet_not_the_whole_ring() {
        let px = render(&glow(Motion::Spin, 255), 0);
        let lit = lit_count(&px);
        assert!(lit > 0, "the comet has to be somewhere");
        assert!(lit < COUNT, "a comet that lights everything is just Breath");
        let widest = (SPIN_TAIL_LEDS + COMET_LEAD_LEDS) as usize;
        assert!(lit <= widest, "comet spans {lit} LEDs, wider than {widest}");
    }

    #[test]
    fn spin_travels_all_the_way_round_and_returns() {
        let g = glow(Motion::Spin, 255);
        let start = render(&g, 0);
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
        let head = px[0].r as u32;
        let behind = px[COUNT - 1].r as u32;
        assert!(head > behind, "head {head} should lead tail {behind}");
    }

    #[test]
    fn alert_is_two_sharp_flashes_then_a_gap() {
        let g = glow(Motion::Alert, 255);
        assert!(total(&render(&g, 0)) > 0, "first flash");
        assert_eq!(total(&render(&g, ALERT_FLASH_MS + 10)), 0, "gap between flashes");
        assert!(total(&render(&g, ALERT_FLASH_MS + ALERT_GAP_MS + 10)) > 0, "second flash");
        assert_eq!(total(&render(&g, ALERT_PERIOD_MS - 10)), 0, "long dark rest");
    }

    #[test]
    fn alert_spends_most_of_its_cycle_dark() {
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
        let mut warm = glow(Motion::Aurora, 255);
        warm.color = Rgb { r: 255, g: 0, b: 0 };
        let mut cool = glow(Motion::Aurora, 255);
        cool.color = Rgb { r: 0, g: 255, b: 0 };
        assert_eq!(render(&warm, 1234), render(&cool, 1234), "aurora must ignore colour");

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
        assert!(SEARCH_PERIOD_MS > SPIN_PERIOD_MS * 2);
    }

    #[test]
    fn speed_scales_a_motion_without_changing_its_shape() {
        let mut fast = glow(Motion::Spin, 255);
        fast.speed = 255;
        let slow = glow(Motion::Spin, 255);
        let half = SPIN_PERIOD_MS / 4;
        let fast_px = render(&fast, half);
        let slow_px = render(&slow, half);
        assert_ne!(fast_px, slow_px, "speed had no effect");
        let widest = (SPIN_TAIL_LEDS + COMET_LEAD_LEDS) as usize;
        assert!(lit_count(&fast_px) <= widest);
        assert!(lit_count(&slow_px) <= widest);
    }

    #[test]
    fn a_zero_speed_does_not_divide_by_zero() {
        let mut g = glow(Motion::Spin, 255);
        g.speed = 0;
        let _ = render(&g, 999);
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
            for t in (0..SEARCH_PERIOD_MS).step_by(37) {
                assert!(
                    total(&render(&dim, t)) <= total(&render(&bright, t)),
                    "{motion:?} exceeded its ceiling at t={t}"
                );
            }
        }
    }

    const FRAME_MS: u32 = 16;

    #[test]
    fn travelling_motions_glide_rather_than_step() {
        const MAX_JUMP: i32 = 90;
        for (motion, period) in
            [(Motion::Spin, SPIN_PERIOD_MS), (Motion::Searching, SEARCH_PERIOD_MS)]
        {
            for speed in [64, NOMINAL_SPEED, 190, 255] {
                let mut g = glow(motion, 255);
                g.speed = speed;
                let mut prev = render(&g, 0);
                let mut t = FRAME_MS;
                while t <= period {
                    let now = render(&g, t);
                    for i in 0..COUNT {
                        let delta = now[i].r as i32 - prev[i].r as i32;
                        assert!(
                            delta.abs() <= MAX_JUMP,
                            "{motion:?} at speed {speed} jumped {delta} on LED {i} at t={t}"
                        );
                    }
                    prev = now;
                    t += FRAME_MS;
                }
            }
        }
    }

    #[test]
    fn the_comet_brightens_before_the_head_reaches_an_led() {
        let span = COUNT as u32 * SUB;
        let head = SUB / 2;
        let w = comet_weight(1, COUNT, head, SPIN_TAIL_LEDS);
        assert!(w > 0, "LED ahead of the head is still dark, so it will snap on");
        assert!(w < 255, "it should not be at full brightness before the head arrives");

        let just_before = comet_weight(1, COUNT, SUB - 1, SPIN_TAIL_LEDS);
        let exactly_on = comet_weight(1, COUNT, SUB, SPIN_TAIL_LEDS);
        assert!(
            (exactly_on as i32 - just_before as i32).abs() <= 2,
            "discontinuity at the head: {just_before} then {exactly_on}"
        );
        let _ = span;
    }

    #[test]
    fn comet_weight_wraps_around_the_ring_seam() {
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
