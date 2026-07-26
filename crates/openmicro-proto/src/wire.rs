//! Framing for the cable link.
//!
//! Over USB-Serial-JTAG the LED/input protocol shares one byte stream with the
//! firmware's own log output — `esp_println` writes to the same TX FIFO. So a
//! reader has to be able to find our frames inside a stream that also contains
//! arbitrary human-readable text, and must never mistake a log line for a frame.
//!
//! ```text
//! 0xF5  len  payload[len]  sum
//! ```
//!
//! - `0xF5` is the start marker. It is not a legal byte anywhere in UTF-8 — not
//!   as a lead byte and not as a continuation byte — so it cannot appear in log
//!   output at all, which is what makes resynchronisation possible.
//! - `len` is the payload length, capped at [`MAX_PAYLOAD`].
//! - `sum` is the sum of the payload bytes, mod 256.
//!
//! The checksum is not for corruption — USB already guards against that. It is
//! there for resynchronisation: after a frame is truncated (a device reset
//! mid-write), the half-frame keeps consuming bytes and will misread the *next*
//! frame's header, and the sum is what makes that misread get discarded instead
//! of rendered as a garbage display. Cost of a truncation is therefore about one
//! extra frame — under two seconds, given the heartbeat.

/// Start-of-frame marker.
///
/// `0xF5` because it cannot occur in valid UTF-8: bytes `0xF5..=0xFF` are not
/// legal lead bytes and are above the continuation range, so no amount of log
/// text can produce one. An earlier attempt used `0x7E`, which is `~` — plainly
/// ASCII, and so plainly able to appear in a log line.
pub const FRAME_START: u8 = 0xF5;

/// Largest payload a frame may carry.
///
/// A postcard-encoded `LedFrame` is around 50 bytes; this leaves room to grow
/// without allowing a bogus length byte to make a reader wait for 255 bytes that
/// will never arrive.
pub const MAX_PAYLOAD: usize = 96;

/// Bytes a frame occupies on the wire for a given payload length.
pub const fn framed_len(payload: usize) -> usize {
    payload + 3
}

/// The checksum for a payload.
pub fn checksum(payload: &[u8]) -> u8 {
    let mut sum: u8 = 0;
    for b in payload {
        sum = sum.wrapping_add(*b);
    }
    sum
}

/// Write `payload` into `out` as a frame, returning the number of bytes written.
///
/// `None` if the payload is too long or `out` is too small — never a partial
/// frame, which a reader could not distinguish from a truncated one.
pub fn encode(payload: &[u8], out: &mut [u8]) -> Option<usize> {
    let need = framed_len(payload.len());
    if payload.len() > MAX_PAYLOAD || out.len() < need {
        return None;
    }
    out[0] = FRAME_START;
    out[1] = payload.len() as u8;
    out[2..2 + payload.len()].copy_from_slice(payload);
    out[2 + payload.len()] = checksum(payload);
    Some(need)
}

/// Incremental frame reader.
///
/// Fed one byte at a time, because that is how both ends receive: the firmware
/// polls a FIFO and the host reads whatever a `read` happened to return. Holding
/// the partial frame here means neither side needs its own buffering.
pub struct Reader {
    buf: [u8; MAX_PAYLOAD],
    state: State,
    len: usize,
    at: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Skipping anything that is not a start marker — log text, usually.
    Hunting,
    /// Marker seen; the next byte is the length.
    Length,
    /// Collecting the payload.
    Payload,
    /// Payload complete; the next byte is the checksum.
    Sum,
}

/// What a byte did to the reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Nothing yet.
    None,
    /// A complete, checksum-valid frame ends at this byte. Read it with
    /// [`Reader::frame`].
    Frame,
    /// A frame was assembled but its checksum did not match, so it was dropped.
    /// Surfaced rather than swallowed so it can be logged — a stream of these
    /// means the framing is out of step, not that the display is idle.
    Bad,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    pub const fn new() -> Self {
        Self { buf: [0; MAX_PAYLOAD], state: State::Hunting, len: 0, at: 0 }
    }

    /// The payload of the frame just completed. Only meaningful straight after
    /// [`Feed::Frame`].
    pub fn frame(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// True while part-way through a frame.
    ///
    /// Lets a caller that multiplexes framed data with plain text on one stream
    /// tell the two apart: a byte fed while this is true belongs to a frame, and
    /// must not also be interpreted as a command.
    pub fn in_frame(&self) -> bool {
        self.state != State::Hunting
    }

    /// Feed one byte.
    pub fn push(&mut self, byte: u8) -> Feed {
        match self.state {
            State::Hunting => {
                if byte == FRAME_START {
                    self.state = State::Length;
                }
                Feed::None
            }
            State::Length => {
                // A length of zero, or one beyond what we can hold, means this
                // marker was not really a frame start. Go back to hunting
                // rather than waiting for bytes that will never come.
                if byte == 0 || byte as usize > MAX_PAYLOAD {
                    self.state = if byte == FRAME_START { State::Length } else { State::Hunting };
                    return Feed::None;
                }
                self.len = byte as usize;
                self.at = 0;
                self.state = State::Payload;
                Feed::None
            }
            State::Payload => {
                self.buf[self.at] = byte;
                self.at += 1;
                if self.at == self.len {
                    self.state = State::Sum;
                }
                Feed::None
            }
            State::Sum => {
                self.state = State::Hunting;
                if byte == checksum(&self.buf[..self.len]) {
                    Feed::Frame
                } else {
                    Feed::Bad
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed every byte, returning the payloads of all complete frames.
    fn read_all(bytes: &[u8]) -> std::vec::Vec<std::vec::Vec<u8>> {
        let mut r = Reader::new();
        let mut out = std::vec::Vec::new();
        for b in bytes {
            if r.push(*b) == Feed::Frame {
                out.push(r.frame().to_vec());
            }
        }
        out
    }

    #[test]
    fn in_frame_marks_the_bytes_that_belong_to_a_frame() {
        // The firmware multiplexes single-byte mode commands with framed data on
        // one stream. Without this, a frame's payload bytes get handed to the
        // command parser as well and it complains about each one.
        let mut r = Reader::new();
        assert!(!r.in_frame(), "idle");
        r.push(FRAME_START);
        assert!(r.in_frame(), "the marker starts a frame");
        r.push(2);
        assert!(r.in_frame(), "length");
        r.push(b'd');
        assert!(r.in_frame(), "payload, even when it looks like a command");
        r.push(b'n');
        assert!(r.in_frame(), "still collecting the checksum");
        r.push(checksum(b"dn"));
        assert!(!r.in_frame(), "back to idle after a complete frame");
    }

    #[test]
    fn a_frame_round_trips() {
        let payload = [1u8, 2, 3, 250];
        let mut buf = [0u8; 32];
        let n = encode(&payload, &mut buf).unwrap();
        assert_eq!(n, framed_len(payload.len()));
        assert_eq!(read_all(&buf[..n]), std::vec![payload.to_vec()]);
    }

    #[test]
    fn log_text_around_a_frame_is_skipped() {
        // The real case: our frames arrive interleaved with esp_println output.
        let payload = [9u8, 8, 7];
        let mut buf = [0u8; 32];
        let n = encode(&payload, &mut buf).unwrap();

        let mut stream = std::vec::Vec::new();
        stream.extend_from_slice(b"alive t=2000ms link=NoDaemon\n");
        stream.extend_from_slice(&buf[..n]);
        stream.extend_from_slice(b"\nleds: awake\n");
        assert_eq!(read_all(&stream), std::vec![payload.to_vec()]);
    }

    #[test]
    fn back_to_back_frames_both_arrive() {
        let mut buf = [0u8; 64];
        let a = encode(&[1, 2], &mut buf).unwrap();
        let mut stream = buf[..a].to_vec();
        let b = encode(&[3, 4, 5], &mut buf).unwrap();
        stream.extend_from_slice(&buf[..b]);
        assert_eq!(read_all(&stream), std::vec![std::vec![1, 2], std::vec![3, 4, 5]]);
    }

    #[test]
    fn a_corrupt_checksum_is_reported_and_dropped() {
        let mut buf = [0u8; 32];
        let n = encode(&[1, 2, 3], &mut buf).unwrap();
        buf[n - 1] ^= 0xFF; // break the sum

        let mut r = Reader::new();
        let mut saw_bad = false;
        for b in &buf[..n] {
            match r.push(*b) {
                Feed::Bad => saw_bad = true,
                Feed::Frame => panic!("a bad frame must not be delivered"),
                Feed::None => {}
            }
        }
        assert!(saw_bad, "a checksum failure must be visible, not silent");
    }

    #[test]
    fn a_truncated_frame_costs_at_most_one_more_then_recovers() {
        // A device reset mid-frame is normal, and length-prefixed framing has an
        // unavoidable consequence: the half-frame keeps consuming bytes, so it
        // eats the *next* frame's header too. What matters is that it recovers
        // rather than wedging. With a 1.5 s heartbeat that is ~3 s of stale
        // display, which is acceptable; a wedged reader would be forever.
        let mut buf = [0u8; 32];
        let n = encode(&[1, 2, 3, 4, 5], &mut buf).unwrap();
        let mut stream = buf[..n - 2].to_vec(); // cut the tail off

        for _ in 0..3 {
            let m = encode(&[7, 7], &mut buf).unwrap();
            stream.extend_from_slice(&buf[..m]);
        }
        let frames = read_all(&stream);
        assert!(
            frames.contains(&std::vec![7, 7]),
            "reader never resynchronised: {frames:?}"
        );
        assert!(frames.len() >= 2, "recovery took more than one frame: {frames:?}");
    }

    #[test]
    fn a_zero_length_marker_is_not_a_frame() {
        // Guards against `0x7E 0x00` in noise leaving the reader stuck.
        assert!(read_all(&[FRAME_START, 0, 0]).is_empty());
    }

    #[test]
    fn an_over_long_length_is_rejected_rather_than_awaited() {
        // A bogus length must not make the reader swallow the next real frame
        // while it waits for bytes that never come.
        let mut stream = std::vec![FRAME_START, 0xFF];
        let mut buf = [0u8; 32];
        let n = encode(&[4, 4], &mut buf).unwrap();
        stream.extend_from_slice(&buf[..n]);
        assert_eq!(read_all(&stream), std::vec![std::vec![4, 4]]);
    }

    #[test]
    fn a_marker_immediately_after_a_bad_length_still_starts_a_frame() {
        // `0x7E 0x7E len ...` — the second marker is the real one.
        let mut buf = [0u8; 32];
        let n = encode(&[5], &mut buf).unwrap();
        let mut stream = std::vec![FRAME_START];
        stream.extend_from_slice(&buf[..n]);
        assert_eq!(read_all(&stream), std::vec![std::vec![5]]);
    }

    #[test]
    fn encoding_refuses_an_oversized_payload_or_a_small_buffer() {
        let big = [0u8; MAX_PAYLOAD + 1];
        let mut buf = [0u8; 256];
        assert!(encode(&big, &mut buf).is_none());

        let mut tiny = [0u8; 3];
        assert!(encode(&[1, 2, 3], &mut tiny).is_none());
    }

    #[test]
    fn a_full_size_payload_fits() {
        let payload = [7u8; MAX_PAYLOAD];
        let mut buf = [0u8; framed_len(MAX_PAYLOAD)];
        let n = encode(&payload, &mut buf).unwrap();
        assert_eq!(read_all(&buf[..n]), std::vec![payload.to_vec()]);
    }

    #[test]
    fn the_marker_cannot_occur_in_utf8_text() {
        // The entire resynchronisation strategy rests on this. 0xF5..=0xFF are
        // not valid UTF-8 lead bytes, and continuation bytes only go to 0xBF.
        assert!(FRAME_START >= 0xF5, "marker must be an illegal UTF-8 byte");
        assert!(
            core::str::from_utf8(&[FRAME_START]).is_err(),
            "the marker is representable in text, so log output could contain it"
        );
    }

    #[test]
    fn a_payload_containing_the_marker_survives() {
        // Length-prefixing rather than escaping means payload bytes are never
        // special. Postcard output will contain 0x7E sooner or later.
        let payload = [FRAME_START, 0, FRAME_START, 0xFF];
        let mut buf = [0u8; 32];
        let n = encode(&payload, &mut buf).unwrap();
        assert_eq!(read_all(&buf[..n]), std::vec![payload.to_vec()]);
    }
}
