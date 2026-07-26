pub const FRAME_START: u8 = 0xF5;

pub const MAX_PAYLOAD: usize = 96;

pub const fn framed_len(payload: usize) -> usize {
    payload + 3
}

pub fn checksum(payload: &[u8]) -> u8 {
    let mut sum: u8 = 0;
    for b in payload {
        sum = sum.wrapping_add(*b);
    }
    sum
}

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

pub struct Reader {
    buf: [u8; MAX_PAYLOAD],
    state: State,
    len: usize,
    at: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Hunting,
    Length,
    Payload,
    Sum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    None,
    Frame,
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

    pub fn frame(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn in_frame(&self) -> bool {
        self.state != State::Hunting
    }

    pub fn push(&mut self, byte: u8) -> Feed {
        match self.state {
            State::Hunting => {
                if byte == FRAME_START {
                    self.state = State::Length;
                }
                Feed::None
            }
            State::Length => {
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
        buf[n - 1] ^= 0xFF;

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
        let mut buf = [0u8; 32];
        let n = encode(&[1, 2, 3, 4, 5], &mut buf).unwrap();
        let mut stream = buf[..n - 2].to_vec();

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
        assert!(read_all(&[FRAME_START, 0, 0]).is_empty());
    }

    #[test]
    fn an_over_long_length_is_rejected_rather_than_awaited() {
        let mut stream = std::vec![FRAME_START, 0xFF];
        let mut buf = [0u8; 32];
        let n = encode(&[4, 4], &mut buf).unwrap();
        stream.extend_from_slice(&buf[..n]);
        assert_eq!(read_all(&stream), std::vec![std::vec![4, 4]]);
    }

    #[test]
    fn a_marker_immediately_after_a_bad_length_still_starts_a_frame() {
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
        assert!(FRAME_START >= 0xF5, "marker must be an illegal UTF-8 byte");
        assert!(
            core::str::from_utf8(&[FRAME_START]).is_err(),
            "the marker is representable in text, so log output could contain it"
        );
    }

    #[test]
    fn a_payload_containing_the_marker_survives() {
        let payload = [FRAME_START, 0, FRAME_START, 0xFF];
        let mut buf = [0u8; 32];
        let n = encode(&payload, &mut buf).unwrap();
        assert_eq!(read_all(&buf[..n]), std::vec![payload.to_vec()]);
    }
}
