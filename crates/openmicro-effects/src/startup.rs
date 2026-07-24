//! The boot animation.
//!
//! Runs once, on power-up, before the agent-state rendering takes over. It has
//! two jobs beyond looking good: it proves the LED chain is alive and correctly
//! ordered (a wrong chain index or a swapped colour order is obvious in a
//! sweep, and invisible in a static colour), and it gives the BLE stack a
//! moment to come up before the first frame arrives.
//!
//! Pure, and integer-only: every pixel is a function of `(t_ms, index)`, so the
//! whole animation is unit-testable and needs no clock, no allocation and no
//! floating point.

use crate::{hsv_to_rgb, scale, Rgb};

/// Total animation length. Long enough to read as deliberate, short enough
/// that nobody waits for it.
pub const DURATION_MS: u32 = 1400;

/// The sweep: a bright head runs the length of the chain, leaving a decaying
/// tail behind it.
const SWEEP_MS: u32 = 700;

/// The settle: the whole chain glows a single hue and fades out, handing over
/// to the real frame at black.
const SETTLE_MS: u32 = DURATION_MS - SWEEP_MS;

/// How many LEDs behind the head still glow. A longer tail reads as smoother
/// motion; beyond about a third of the chain it just looks like a flash.
const TAIL: u32 = 4;

/// Hue the sweep starts at, in the 0..=255 hue space `hsv_to_rgb` uses. A cyan
/// through violet ramp, which matches the interface's own accent colour.
const HUE_START: u8 = 130;
const HUE_SPAN: u8 = 60;

/// Colour of `index` at `t_ms`, for a chain of `count` LEDs.
///
/// Returns black once the animation is over, so a caller that keeps rendering
/// past [`DURATION_MS`] simply sees the chain go dark.
pub fn pixel(t_ms: u32, index: usize, count: usize) -> Rgb {
    const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };
    if count == 0 || index >= count || t_ms >= DURATION_MS {
        return BLACK;
    }

    let hue = HUE_START.wrapping_add(((index as u32 * HUE_SPAN as u32) / count as u32) as u8);

    if t_ms < SWEEP_MS {
        // Head position in "LED units", scaled so it runs one LED past the end
        // and the last pixel gets its full moment.
        let head = (t_ms * (count as u32 + TAIL)) / SWEEP_MS;
        let idx = index as u32;
        if head < idx {
            return BLACK; // the sweep has not arrived yet
        }
        let behind = head - idx;
        if behind > TAIL {
            return BLACK; // the tail has passed
        }
        // Linear falloff from the head back through the tail.
        let brightness = 255 - (behind * 255 / (TAIL + 1)) as u8;
        scale(hsv_to_rgb(hue, 255, 255), brightness)
    } else {
        // Settle: everything lit, fading linearly to black.
        let elapsed = t_ms - SWEEP_MS;
        let remaining = SETTLE_MS - elapsed;
        let brightness = (remaining * 255 / SETTLE_MS) as u8;
        scale(hsv_to_rgb(hue, 255, 255), brightness)
    }
}

/// Fill `out` with the frame at `t_ms`. Convenience for the render loop.
pub fn frame(t_ms: u32, out: &mut [Rgb]) {
    let count = out.len();
    for (i, px) in out.iter_mut().enumerate() {
        *px = pixel(t_ms, i, count);
    }
}

/// Whether the animation is still running at `t_ms`.
pub fn is_running(t_ms: u32) -> bool {
    t_ms < DURATION_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn lit(t_ms: u32, count: usize) -> std::vec::Vec<usize> {
        (0..count)
            .filter(|i| {
                let p = pixel(t_ms, *i, count);
                p.r != 0 || p.g != 0 || p.b != 0
            })
            .collect()
    }

    #[test]
    fn the_sweep_starts_at_the_first_led() {
        let first = lit(0, 13);
        assert_eq!(first, vec![0], "only the head is lit at t=0");
    }

    #[test]
    fn the_head_moves_forward_over_time() {
        // The brightest lit index should never move backwards.
        let mut furthest = 0;
        for t in (0..SWEEP_MS).step_by(25) {
            if let Some(max) = lit(t, 13).into_iter().max() {
                assert!(max >= furthest, "head went backwards at t={t}");
                furthest = max;
            }
        }
        assert_eq!(furthest, 12, "the sweep must reach the last LED");
    }

    #[test]
    fn the_tail_is_dimmer_than_the_head() {
        // Pick a moment where head and tail are both on-chain.
        let t = SWEEP_MS / 2;
        let on = lit(t, 13);
        assert!(on.len() > 1, "expected a tail at t={t}: {on:?}");
        let head = *on.iter().max().unwrap();
        let tail = *on.iter().min().unwrap();
        let brightness = |i: usize| {
            let p = pixel(t, i, 13);
            p.r as u32 + p.g as u32 + p.b as u32
        };
        assert!(
            brightness(head) > brightness(tail),
            "head {head} should outshine tail {tail}"
        );
    }

    #[test]
    fn the_settle_lights_the_whole_chain_then_fades_to_black() {
        let just_after_sweep = lit(SWEEP_MS + 10, 13);
        assert_eq!(just_after_sweep.len(), 13, "settle lights everything");

        let early = pixel(SWEEP_MS + 10, 6, 13);
        let late = pixel(DURATION_MS - 50, 6, 13);
        let sum = |p: Rgb| p.r as u32 + p.g as u32 + p.b as u32;
        assert!(sum(early) > sum(late), "the settle must fade, not hold");
    }

    #[test]
    fn everything_is_black_once_it_is_over() {
        assert!(lit(DURATION_MS, 13).is_empty());
        assert!(lit(DURATION_MS + 10_000, 13).is_empty());
        assert!(!is_running(DURATION_MS));
        assert!(is_running(DURATION_MS - 1));
    }

    #[test]
    fn it_works_on_either_chain_length() {
        // 13 per-key and 8 underglow are the two real cases; neither may panic
        // or leave the chain permanently lit.
        for count in [8usize, 13] {
            for t in (0..DURATION_MS + 100).step_by(37) {
                let mut buf = vec![Rgb { r: 0, g: 0, b: 0 }; count];
                frame(t, &mut buf);
            }
            assert!(lit(DURATION_MS, count).is_empty(), "count {count} left lit");
        }
    }

    #[test]
    fn an_empty_chain_is_handled() {
        let mut none: [Rgb; 0] = [];
        frame(100, &mut none);
        assert_eq!(pixel(100, 0, 0), Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn colours_vary_along_the_chain() {
        // A single-colour sweep would hide a wrong chain order; the hue ramp is
        // what makes a misordered chain visible.
        let a = pixel(SWEEP_MS + 10, 0, 13);
        let b = pixel(SWEEP_MS + 10, 12, 13);
        assert_ne!(a, b, "the two ends should not be the same colour");
    }
}
