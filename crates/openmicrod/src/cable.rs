//! Driving the device over the USB cable.
//!
//! The transport that actually works. BLE needs a GATT server the firmware does
//! not yet have; this needs only the USB-Serial-JTAG console the firmware already
//! exposes for its logs, and it has no pairing to lose and no link to drop.
//!
//! Both directions share one byte stream with the firmware's log output, so
//! everything is `wire`-framed (see `openmicro_proto::wire`) and the reader skips
//! anything that is not a frame.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openmicro_proto::{wire, InputEvent, LedFrame};
use tokio::sync::mpsc::UnboundedSender;

use crate::device::DeviceLink;

/// Serial ports the device might appear on, in order.
fn candidate_ports() -> Vec<PathBuf> {
    (0..4).map(|i| PathBuf::from(format!("/dev/ttyACM{i}"))).collect()
}

/// The first candidate that exists.
pub fn find_port() -> Option<PathBuf> {
    candidate_ports().into_iter().find(|p| p.exists())
}

/// A device reached over the cable.
pub struct CableDevice {
    port: PathBuf,
    /// Shared so the reader thread and the writer can hold the same handle: the
    /// character device must be opened once, or the two halves fight over it.
    handle: Arc<Mutex<std::fs::File>>,
    last: LedFrame,
    /// Frames that failed to write. Surfaced in logs rather than silently
    /// dropped, because a cable that has come loose looks exactly like an idle
    /// desk otherwise.
    pub write_errors: u64,
}

impl CableDevice {
    /// Open the device and start reading input events.
    ///
    /// `input_tx` receives every decoded [`InputEvent`]; the daemon routes them.
    pub fn open(input_tx: UnboundedSender<InputEvent>) -> Result<Self, String> {
        let port = find_port().ok_or_else(|| "no serial port found".to_string())?;
        Self::open_at(&port, input_tx)
    }

    pub fn open_at(port: &Path, input_tx: UnboundedSender<InputEvent>) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(port)
            .map_err(|e| format!("can't open {}: {e}", port.display()))?;

        // Put the tty into raw mode before a single byte moves.
        //
        // This is not optional and it is not obvious. A tty in its default line
        // discipline *rewrites the byte stream*: `ONLCR` turns every 0x0A into
        // CRLF, `IXON` eats 0x11/0x13 as flow control, and `ICANON` holds input
        // until a newline. ASCII mode commands survive all of that, which is
        // exactly why the first version appeared to work — but a postcard-encoded
        // frame contains 0x0A sooner or later, and every such frame was silently
        // corrupted. The device simply never saw a valid frame.
        // A plain file is not a tty, which only happens in tests; the framing is
        // what those exercise, so a failure here is not fatal for them.
        if let Err(e) = make_raw(&file) {
            if port.to_string_lossy().starts_with("/dev/tty") {
                return Err(e);
            }
        }

        let handle = Arc::new(Mutex::new(file));

        // A blocking thread rather than async: reads from a tty are blocking
        // reads on a character device, and tokio's file I/O would use a blocking
        // pool for them anyway. One dedicated thread is simpler and its
        // lifetime is the daemon's.
        let reader_handle = handle.clone();
        std::thread::spawn(move || read_loop(reader_handle, input_tx));

        Ok(Self { port: port.to_path_buf(), handle, last: LedFrame::BLANK, write_errors: 0 })
    }

    pub fn port(&self) -> &Path {
        &self.port
    }
}

/// Put a tty into raw mode: no input canonicalisation, no output translation,
/// no flow control, no echo.
///
/// Reads return whatever is available rather than waiting for a line, which is
/// what lets the reader thread make progress on partial frames.
fn make_raw(file: &std::fs::File) -> Result<(), String> {
    use std::os::fd::AsRawFd;

    let fd = file.as_raw_fd();
    // SAFETY: `fd` is a live descriptor owned by `file` for the duration of this
    // call, and `termios` is a plain POD struct that tcgetattr fully initialises
    // before cfmakeraw/tcsetattr read it.
    unsafe {
        let mut tio: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut tio) != 0 {
            return Err("tcgetattr failed; is this a tty?".to_string());
        }
        libc::cfmakeraw(&mut tio);
        // Return from `read` as soon as any byte is available, and never block
        // indefinitely: the reader thread must stay responsive to shutdown.
        tio.c_cc[libc::VMIN] = 0;
        tio.c_cc[libc::VTIME] = 1; // tenths of a second
        if libc::tcsetattr(fd, libc::TCSANOW, &tio) != 0 {
            return Err("tcsetattr failed".to_string());
        }
    }
    Ok(())
}

/// Read framed input events forever, discarding log text.
fn read_loop(handle: Arc<Mutex<std::fs::File>>, input_tx: UnboundedSender<InputEvent>) {
    let mut reader = wire::Reader::new();
    let mut buf = [0u8; 256];
    loop {
        let read = {
            let mut file = match handle.lock() {
                Ok(f) => f,
                // The writer panicked while holding the lock; nothing useful is
                // left to read from.
                Err(_) => return,
            };
            file.read(&mut buf)
        };
        match read {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Ok(n) => {
                for byte in &buf[..n] {
                    if reader.push(*byte) == wire::Feed::Frame {
                        match InputEvent::decode(reader.frame()) {
                            Ok(ev) => {
                                // A closed receiver means the daemon is shutting
                                // down, so stop rather than spin.
                                if input_tx.send(ev).is_err() {
                                    return;
                                }
                            }
                            // Not an input event — the firmware may frame other
                            // things later. Ignore rather than treat as fatal.
                            Err(_) => {}
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                // The device went away. Sleep rather than spin; the daemon's
                // heartbeat writes will report the real problem.
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
}

#[async_trait]
impl DeviceLink for CableDevice {
    async fn set_leds(&mut self, frame: &LedFrame) {
        self.last = *frame;
        let Ok(payload) = frame.encode() else {
            eprintln!("openmicrod: could not encode an LED frame");
            return;
        };
        let mut framed = vec![0u8; wire::framed_len(payload.len())];
        let Some(n) = wire::encode(&payload, &mut framed) else {
            eprintln!("openmicrod: LED frame too large to send ({} bytes)", payload.len());
            return;
        };

        let result = self
            .handle
            .lock()
            .map_err(|_| "serial handle poisoned".to_string())
            .and_then(|mut f| {
                f.write_all(&framed[..n]).and_then(|_| f.flush()).map_err(|e| e.to_string())
            });

        if let Err(e) = result {
            // Only the first failure is loud: a detached cable would otherwise
            // print every heartbeat, forever.
            if self.write_errors == 0 {
                eprintln!("openmicrod: cable write failed ({e}); is the device still plugged in?");
            }
            self.write_errors += 1;
        } else {
            self.write_errors = 0;
        }
    }

    fn last_frame(&self) -> LedFrame {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmicro_proto::{AgentKind, Effect, LedSlot};

    /// A frame with something in it, so encoding is not trivially empty.
    fn a_frame() -> LedFrame {
        let mut f = LedFrame::BLANK;
        f.brightness = 200;
        f.slots[0] = LedSlot {
            color: AgentKind::Claude.brand(),
            effect: Effect::Pulse,
            brightness: 200,
        };
        f.actions.approve = true;
        f
    }

    #[test]
    fn a_led_frame_fits_in_one_wire_frame() {
        // The firmware's RX FIFO is 64 bytes and MAX_PAYLOAD is 96. If a frame
        // ever outgrew the payload limit it would be silently undeliverable, so
        // pin the actual size down.
        let payload = a_frame().encode().unwrap();
        assert!(
            payload.len() <= wire::MAX_PAYLOAD,
            "an LedFrame is {} bytes, over the {} limit",
            payload.len(),
            wire::MAX_PAYLOAD
        );
    }

    #[test]
    fn a_frame_survives_the_round_trip_through_framing() {
        let frame = a_frame();
        let payload = frame.encode().unwrap();
        let mut framed = vec![0u8; wire::framed_len(payload.len())];
        let n = wire::encode(&payload, &mut framed).unwrap();

        let mut reader = wire::Reader::new();
        let mut got = None;
        for b in &framed[..n] {
            if reader.push(*b) == wire::Feed::Frame {
                got = Some(LedFrame::decode(reader.frame()).unwrap());
            }
        }
        assert_eq!(got, Some(frame));
    }

    #[test]
    fn input_events_survive_the_round_trip() {
        for ev in [
            InputEvent::Key { id: openmicro_proto::layout::APPROVE_KEY, pressed: true },
            InputEvent::Encoder { delta: -3 },
            InputEvent::Joystick { dir: 5 },
        ] {
            let payload = ev.encode().unwrap();
            let mut framed = vec![0u8; wire::framed_len(payload.len())];
            let n = wire::encode(&payload, &mut framed).unwrap();
            let mut reader = wire::Reader::new();
            let mut got = None;
            for b in &framed[..n] {
                if reader.push(*b) == wire::Feed::Frame {
                    got = Some(InputEvent::decode(reader.frame()).unwrap());
                }
            }
            assert_eq!(got, Some(ev));
        }
    }

    #[test]
    fn opening_a_missing_port_is_an_error() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = match CableDevice::open_at(Path::new("/definitely/not/a/port"), tx) {
            Err(e) => e,
            Ok(_) => panic!("opening a missing port must fail"),
        };
        assert!(err.contains("can't open"), "unhelpful error: {err}");
    }

    #[tokio::test]
    async fn writing_to_a_plain_file_emits_a_readable_frame() {
        let dir = std::env::temp_dir().join(format!("om-cable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("port");
        std::fs::write(&path, b"").unwrap();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut dev = CableDevice::open_at(&path, tx).unwrap();
        let frame = a_frame();
        dev.set_leds(&frame).await;
        assert_eq!(dev.write_errors, 0, "a writable file must not report errors");
        assert_eq!(dev.last_frame(), frame);

        // And what landed on disk must decode back to the same frame.
        let bytes = std::fs::read(&path).unwrap();
        let mut reader = wire::Reader::new();
        let mut got = None;
        for b in &bytes {
            if reader.push(*b) == wire::Feed::Frame {
                got = Some(LedFrame::decode(reader.frame()).unwrap());
            }
        }
        assert_eq!(got, Some(frame), "the bytes written were not a valid frame");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn candidate_ports_are_acm_devices_in_order() {
        let ports = candidate_ports();
        assert_eq!(ports[0], PathBuf::from("/dev/ttyACM0"));
        assert!(ports.len() > 1, "one candidate is not enough if the device moves");
    }
}
