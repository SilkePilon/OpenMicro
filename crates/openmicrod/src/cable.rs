use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openmicro_proto::{wire, InputEvent, LedFrame};
use tokio::sync::mpsc::UnboundedSender;

use crate::device::DeviceLink;

fn candidate_ports() -> Vec<PathBuf> {
    (0..4).map(|i| PathBuf::from(format!("/dev/ttyACM{i}"))).collect()
}

pub fn find_port() -> Option<PathBuf> {
    candidate_ports().into_iter().find(|p| p.exists())
}

pub struct CableDevice {
    port: PathBuf,
    handle: Arc<Mutex<std::fs::File>>,
    last: LedFrame,
    pub write_errors: u64,
}

impl CableDevice {
    pub fn open(input_tx: UnboundedSender<InputEvent>) -> Result<Self, String> {
        let port = find_port().ok_or_else(|| "no serial port found".to_string())?;
        Self::open_at(&port, input_tx)
    }

    pub fn open_at(port: &Path, input_tx: UnboundedSender<InputEvent>) -> Result<Self, String> {
        use std::os::unix::fs::OpenOptionsExt;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(port)
            .map_err(|e| format!("can't open {}: {e}", port.display()))?;

        if let Err(e) = make_raw(&file) {
            if port.to_string_lossy().starts_with("/dev/tty") {
                return Err(e);
            }
        }

        let handle = Arc::new(Mutex::new(file));

        let reader_handle = handle.clone();
        std::thread::spawn(move || read_loop(reader_handle, input_tx));

        Ok(Self { port: port.to_path_buf(), handle, last: LedFrame::BLANK, write_errors: 0 })
    }

    pub fn port(&self) -> &Path {
        &self.port
    }
}

fn make_raw(file: &std::fs::File) -> Result<(), String> {
    use std::os::fd::AsRawFd;

    let fd = file.as_raw_fd();
    unsafe {
        let mut tio: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut tio) != 0 {
            return Err("tcgetattr failed; is this a tty?".to_string());
        }
        libc::cfmakeraw(&mut tio);
        tio.c_cc[libc::VMIN] = 0;
        tio.c_cc[libc::VTIME] = 1;
        if libc::tcsetattr(fd, libc::TCSANOW, &tio) != 0 {
            return Err("tcsetattr failed".to_string());
        }
    }
    Ok(())
}

fn read_loop(handle: Arc<Mutex<std::fs::File>>, input_tx: UnboundedSender<InputEvent>) {
    let mut reader = wire::Reader::new();
    let mut buf = [0u8; 256];
    loop {
        let read = {
            let mut file = match handle.lock() {
                Ok(f) => f,
                Err(_) => return,
            };
            file.read(&mut buf)
        };
        match read {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Ok(n) => {
                for byte in &buf[..n] {
                    if reader.push(*byte) == wire::Feed::Frame {
                        if let Ok(ev) = InputEvent::decode(reader.frame()) {
                            if input_tx.send(ev).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
}

const WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

fn write_all_before(
    file: &mut std::fs::File,
    mut buf: &[u8],
    deadline: std::time::Instant,
) -> std::io::Result<()> {
    while !buf.is_empty() {
        match file.write(buf) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => buf = &buf[n..],
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                if std::time::Instant::now() >= deadline {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(e),
        }
    }
    file.flush()
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
                let deadline = std::time::Instant::now() + WRITE_DEADLINE;
                write_all_before(&mut f, &framed[..n], deadline).map_err(|e| e.to_string())
            });

        if let Err(e) = result {
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
    fn a_stalled_write_errors_at_the_deadline_instead_of_blocking() {
        use std::os::fd::FromRawFd;

        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) }, 0);
        let _read_end = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        let mut write_end = unsafe { std::fs::File::from_raw_fd(fds[1]) };

        let filler = [0u8; 4096];
        loop {
            match write_end.write(&filler) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("unexpected error filling the pipe: {e}"),
            }
        }

        let started = std::time::Instant::now();
        let deadline = started + std::time::Duration::from_millis(50);
        let err = write_all_before(&mut write_end, b"stuck", deadline)
            .expect_err("a full pipe must error out, not block");
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the deadline did not bound the write"
        );
    }

    #[test]
    fn candidate_ports_are_acm_devices_in_order() {
        let ports = candidate_ports();
        assert_eq!(ports[0], PathBuf::from("/dev/ttyACM0"));
        assert!(ports.len() > 1, "one candidate is not enough if the device moves");
    }
}
