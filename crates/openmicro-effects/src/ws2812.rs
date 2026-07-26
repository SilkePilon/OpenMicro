use crate::Rgb;

pub const SPI_HZ: u32 = 2_500_000;

pub const SPI_BITS_PER_BIT: usize = 3;

const PATTERN_ZERO: u32 = 0b100;
const PATTERN_ONE: u32 = 0b110;

pub const BYTES_PER_PIXEL: usize = 24 * SPI_BITS_PER_BIT / 8;

pub const RESET_BYTES: usize = 20;

pub const fn buffer_len(pixel_count: usize) -> usize {
    pixel_count * BYTES_PER_PIXEL + RESET_BYTES
}

fn encode_byte(value: u8, out: &mut [u8; 3]) {
    let mut word: u32 = 0;
    for bit in (0..8).rev() {
        let pattern = if (value >> bit) & 1 == 1 { PATTERN_ONE } else { PATTERN_ZERO };
        word = (word << 3) | pattern;
    }
    out[0] = (word >> 16) as u8;
    out[1] = (word >> 8) as u8;
    out[2] = word as u8;
}

pub fn encode(pixels: &[Rgb], out: &mut [u8]) -> Option<usize> {
    let needed = buffer_len(pixels.len());
    if out.len() < needed {
        return None;
    }
    let mut at = 0;
    for px in pixels {
        for value in [px.g, px.r, px.b] {
            let mut chunk = [0u8; 3];
            encode_byte(value, &mut chunk);
            out[at..at + 3].copy_from_slice(&chunk);
            at += 3;
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
        let mut out = [0u8; 3];
        encode_byte(0b0000_0000, &mut out);
        assert_eq!(spi_bits(&out)[..3], [1, 0, 0], "a zero is one high SPI bit");

        encode_byte(0b1111_1111, &mut out);
        assert_eq!(spi_bits(&out)[..3], [1, 1, 0], "a one is two high SPI bits");
    }

    #[test]
    fn colour_bits_are_emitted_most_significant_first() {
        let mut out = [0u8; 3];
        encode_byte(0b1000_0000, &mut out);
        let bits = spi_bits(&out);
        assert_eq!(&bits[0..3], &[1, 1, 0], "the MSB is the first bit on the wire");
        assert_eq!(&bits[3..6], &[1, 0, 0], "the next bit is a zero");

        encode_byte(0b0000_0001, &mut out);
        let bits = spi_bits(&out);
        assert_eq!(&bits[0..3], &[1, 0, 0]);
        assert_eq!(&bits[21..24], &[1, 1, 0], "the LSB is the last bit on the wire");
    }

    #[test]
    fn pixels_go_out_in_grb_order() {
        let red = Rgb { r: 255, g: 0, b: 0 };
        let mut buf = [0u8; buffer_len(1)];
        encode(&[red], &mut buf).unwrap();

        let green_group = &buf[0..3];
        let red_group = &buf[3..6];
        let blue_group = &buf[6..9];
        assert_eq!(green_group, &[0x92, 0x49, 0x24], "green is zero: {green_group:?}");
        assert_eq!(red_group, &[0xDB, 0x6D, 0xB6], "red is full: {red_group:?}");
        assert_eq!(blue_group, &[0x92, 0x49, 0x24], "blue is zero: {blue_group:?}");
    }

    #[test]
    fn every_frame_ends_with_a_latch_long_enough_to_reset() {
        let mut buf = [0u8; buffer_len(2)];
        let written = encode(&[Rgb { r: 1, g: 2, b: 3 }; 2], &mut buf).unwrap();
        assert_eq!(written, buffer_len(2));
        assert!(buf[written - RESET_BYTES..written].iter().all(|b| *b == 0));

        let low_ns = RESET_BYTES as u64 * 8 * 1_000_000_000 / SPI_HZ as u64;
        assert!(low_ns >= 50_000, "reset gap is only {low_ns} ns");
    }

    #[test]
    fn the_bit_patterns_land_inside_the_ws2812b_timing_windows() {
        let bit_ns = 1_000_000_000 / SPI_HZ as u64;
        let t0h = bit_ns;
        let t1h = bit_ns * 2;
        let period = bit_ns * SPI_BITS_PER_BIT as u64;
        assert!((250..=550).contains(&t0h), "T0H {t0h} ns outside 250..550");
        assert!((650..=950).contains(&t1h), "T1H {t1h} ns outside 650..950");
        assert!((1100..=1350).contains(&period), "bit period {period} ns off 1.25us");
    }

    #[test]
    fn encoding_refuses_a_buffer_that_is_too_small() {
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
