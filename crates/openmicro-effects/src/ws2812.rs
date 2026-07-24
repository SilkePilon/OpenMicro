//! Encoding WS2812 pixel data into an SPI bit stream.
//!
//! The Creator Micro 2 drives both of its LED chains from SPI hosts, not from
//! the RMT peripheral — that is what the stock firmware does, recovered from
//! its image (see `docs/hardware/creator-micro-2-pinout-findings.md`). SPI has
//! no notion of a WS2812 bit, so each one is spelled out as a fixed pattern of
//! SPI bits whose high time encodes the value:
//!
//! ```text
//! SPI clock 3.2 MHz  ->  one SPI bit = 312.5 ns
//! WS2812 bit         =  4 SPI bits  = 1.25 us   (the WS2812 bit period)
//!
//! '0' -> 1000   312.5 ns high, 937.5 ns low
//! '1' -> 1110   937.5 ns high, 312.5 ns low
//! ```
//!
//! Both high times land inside the WS2812B datasheet windows (T0H 250–550 ns,
//! T1H 650–950 ns), which is why this clock and this pattern go together —
//! changing one without the other silently produces wrong colours or nothing
//! at all.
//!
//! This lives here, rather than in the firmware crate, because it is pure
//! byte-shuffling and therefore the one part of the LED path that can be
//! tested on a host. The firmware just hands the resulting buffer to SPI.

use crate::Rgb;

/// SPI clock this encoding assumes. Driving the bus at any other rate breaks
/// the timings above.
pub const SPI_HZ: u32 = 3_200_000;

/// SPI bits emitted per WS2812 bit.
pub const SPI_BITS_PER_BIT: usize = 4;

/// One WS2812 bit's worth of SPI bits, for a zero and for a one.
const PATTERN_ZERO: u8 = 0b1000;
const PATTERN_ONE: u8 = 0b1110;

/// Encoded bytes per pixel: 24 colour bits x 4 SPI bits = 96 bits.
pub const BYTES_PER_PIXEL: usize = 24 * SPI_BITS_PER_BIT / 8;

/// Trailing low bytes that make up the inter-frame reset.
///
/// WS2812 latches when the line is held low for >= 50 us. At 3.2 MHz one byte
/// of zeros is 2.5 us, so 24 bytes is 60 us — comfortably over, and cheap.
pub const RESET_BYTES: usize = 24;

/// Buffer size needed to encode `pixel_count` pixels plus the reset gap.
pub const fn buffer_len(pixel_count: usize) -> usize {
    pixel_count * BYTES_PER_PIXEL + RESET_BYTES
}

/// Encode one colour byte into four SPI bytes (two WS2812 bits per SPI byte).
fn encode_byte(value: u8, out: &mut [u8; 4]) {
    for (i, chunk) in out.iter_mut().enumerate() {
        // Most significant bit first, two colour bits per output byte.
        let hi_bit = (value >> (7 - i * 2)) & 1;
        let lo_bit = (value >> (6 - i * 2)) & 1;
        let hi = if hi_bit == 1 { PATTERN_ONE } else { PATTERN_ZERO };
        let lo = if lo_bit == 1 { PATTERN_ONE } else { PATTERN_ZERO };
        *chunk = (hi << 4) | lo;
    }
}

/// Encode `pixels` into `out`, returning how many bytes were written.
///
/// Colour order is **GRB**, which is what the WS2812 family expects on the
/// wire and what the vendor firmware's `led_strip` default produces. `out`
/// must be at least [`buffer_len`] bytes; anything beyond the written length
/// is left alone, so a caller can reuse one oversized buffer.
///
/// Returns `None` when the buffer is too small rather than writing a partial,
/// un-latched frame.
pub fn encode(pixels: &[Rgb], out: &mut [u8]) -> Option<usize> {
    let needed = buffer_len(pixels.len());
    if out.len() < needed {
        return None;
    }
    let mut at = 0;
    for px in pixels {
        // Wire order is green, red, blue — not RGB.
        for value in [px.g, px.r, px.b] {
            let mut chunk = [0u8; 4];
            encode_byte(value, &mut chunk);
            out[at..at + 4].copy_from_slice(&chunk);
            at += 4;
        }
    }
    for byte in &mut out[at..at + RESET_BYTES] {
        *byte = 0;
    }
    Some(at + RESET_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    /// Expand encoded SPI bytes back into the bit pattern, so tests can assert
    /// on what the LED actually sees rather than on byte trivia.
    fn spi_bits(bytes: &[u8]) -> std::vec::Vec<u8> {
        let mut bits = vec![];
        for b in bytes {
            for i in (0..8).rev() {
                bits.push((b >> i) & 1);
            }
        }
        bits
    }

    #[test]
    fn a_zero_bit_is_short_high_and_a_one_bit_is_long_high() {
        let mut out = [0u8; 4];
        encode_byte(0b0000_0000, &mut out);
        assert_eq!(spi_bits(&out)[..4], [1, 0, 0, 0], "a zero is one high SPI bit");

        encode_byte(0b1111_1111, &mut out);
        assert_eq!(spi_bits(&out)[..4], [1, 1, 1, 0], "a one is three high SPI bits");
    }

    #[test]
    fn colour_bits_are_emitted_most_significant_first() {
        let mut out = [0u8; 4];
        encode_byte(0b1000_0000, &mut out);
        let bits = spi_bits(&out);
        assert_eq!(&bits[0..4], &[1, 1, 1, 0], "the MSB is the first bit on the wire");
        assert_eq!(&bits[4..8], &[1, 0, 0, 0], "the next bit is a zero");

        encode_byte(0b0000_0001, &mut out);
        let bits = spi_bits(&out);
        assert_eq!(&bits[0..4], &[1, 0, 0, 0]);
        assert_eq!(&bits[28..32], &[1, 1, 1, 0], "the LSB is the last bit on the wire");
    }

    #[test]
    fn pixels_go_out_in_grb_order() {
        // Pure red must appear in the *second* byte group, not the first.
        let red = Rgb { r: 255, g: 0, b: 0 };
        let mut buf = [0u8; buffer_len(1)];
        encode(&[red], &mut buf).unwrap();

        let green_group = &buf[0..4];
        let red_group = &buf[4..8];
        let blue_group = &buf[8..12];
        assert!(green_group.iter().all(|b| *b == 0x88), "green is zero: {green_group:?}");
        assert!(red_group.iter().all(|b| *b == 0xEE), "red is full: {red_group:?}");
        assert!(blue_group.iter().all(|b| *b == 0x88), "blue is zero: {blue_group:?}");
    }

    #[test]
    fn every_frame_ends_with_a_latch_long_enough_to_reset() {
        let mut buf = [0u8; buffer_len(2)];
        let written = encode(&[Rgb { r: 1, g: 2, b: 3 }; 2], &mut buf).unwrap();
        assert_eq!(written, buffer_len(2));
        assert!(buf[written - RESET_BYTES..written].iter().all(|b| *b == 0));

        // The datasheet wants >= 50us of low; check the arithmetic holds rather
        // than trusting the constant.
        let low_ns = RESET_BYTES as u64 * 8 * 1_000_000_000 / SPI_HZ as u64;
        assert!(low_ns >= 50_000, "reset gap is only {low_ns} ns");
    }

    #[test]
    fn the_bit_patterns_land_inside_the_ws2812b_timing_windows() {
        // This is the check that stops someone "optimising" the clock later.
        let bit_ns = 1_000_000_000 / SPI_HZ as u64;
        let t0h = bit_ns; // one high SPI bit
        let t1h = bit_ns * 3; // three high SPI bits
        let period = bit_ns * SPI_BITS_PER_BIT as u64;
        assert!((250..=550).contains(&t0h), "T0H {t0h} ns outside 250..550");
        assert!((650..=950).contains(&t1h), "T1H {t1h} ns outside 650..950");
        assert!((1150..=1350).contains(&period), "bit period {period} ns off 1.25us");
    }

    #[test]
    fn encoding_refuses_a_buffer_that_is_too_small() {
        // A partial frame would leave the strip un-latched showing garbage.
        let mut small = [0u8; 4];
        assert!(encode(&[Rgb { r: 0, g: 0, b: 0 }], &mut small).is_none());
    }

    #[test]
    fn an_empty_frame_still_emits_the_latch() {
        let mut buf = [0u8; buffer_len(0)];
        assert_eq!(encode(&[], &mut buf), Some(RESET_BYTES));
    }

    #[test]
    fn buffer_len_matches_what_encoding_writes() {
        for count in [1usize, 8, 13, 21] {
            let mut buf = std::vec![0u8; buffer_len(count)];
            let pixels = std::vec![Rgb { r: 9, g: 8, b: 7 }; count];
            assert_eq!(encode(&pixels, &mut buf), Some(buffer_len(count)), "{count} pixels");
        }
    }
}
