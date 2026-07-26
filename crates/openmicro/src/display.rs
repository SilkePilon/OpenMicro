//! Switching the device's display mode from the TUI.
//!
//! # Why serial and not the daemon
//!
//! The obvious route would be a `Command` to the daemon, which would pass it to
//! the device. That does not work yet: the daemon reaches the device over BLE,
//! and the firmware's GATT server is still a sketch, so nothing the daemon sends
//! arrives. The firmware *does* expose USB-Serial-JTAG for its logs, and the RX
//! half of that is free — so a short command down the same port switches modes
//! on an already-flashed device, with no BLE and no rebuild.
//!
//! Sharing one stream with the log output has two consequences that everything
//! here is shaped by: the port must be opened raw and non-blocking, and commands
//! must carry a prefix. See [`open_raw`] and [`COMMAND_PREFIX`].
//!
//! When the GATT server lands this should move onto the daemon path, and this
//! module becomes the fallback for a device with no daemon.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// What the device's LEDs should be doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Real state: host frames, or the local fallback animations.
    Normal,
    /// Walk every state the board can show.
    Demo,
    /// Light one LED at a time, logging its chain index.
    Identify,
}

/// Every command byte must be preceded by this one.
///
/// The prefix is not decoration, and dropping it breaks the device. This port
/// carries the firmware's log output *and* its command input, and a tty in its
/// default line discipline **echoes whatever it receives straight back out** —
/// so every log line the firmware printed arrived back at the firmware as
/// input. With bare letters as commands that is a feedback loop: `link:`
/// contains `i` (identify) and `n` (normal), and any line containing a `d` put
/// the board into demo mode on its own. A byte the logs never emit ends it.
///
/// Must match `COMMAND_PREFIX` in `firmware/src/main.rs`.
pub const COMMAND_PREFIX: u8 = b'!';

impl Mode {
    /// The command byte the firmware expects. Must match
    /// `handle_serial_command` in `firmware/src/main.rs`.
    pub fn command(self) -> u8 {
        match self {
            Mode::Normal => b'n',
            Mode::Demo => b'd',
            Mode::Identify => b'i',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "Normal",
            Mode::Demo => "Demo",
            Mode::Identify => "Identify LEDs",
        }
    }
}

/// Serial ports the device might be on, most likely first.
///
/// The ESP32-S3's USB-Serial-JTAG shows up as an ACM device. Only a handful ever
/// exist, so an ordered guess beats depending on a port-enumeration crate.
fn candidate_ports() -> Vec<PathBuf> {
    (0..4).map(|i| PathBuf::from(format!("/dev/ttyACM{i}"))).collect()
}

/// The first candidate port that exists.
pub fn find_port() -> Option<PathBuf> {
    candidate_ports().into_iter().find(|p| p.exists())
}

/// Open a serial port in raw, non-blocking mode.
///
/// Both properties are load-bearing:
///
/// - **Non-blocking.** A `read` on a tty with nothing to say blocks forever. The
///   first version of [`firmware_answers_on`] looped `while now < deadline` around
///   a blocking read, so the deadline was only ever checked *between* reads and
///   the first read never returned. A device sitting in the ROM bootloader is
///   silent by definition, which meant the identify probe hung the setup wizard
///   at "Starting the new firmware" — the one step whose job was to get the
///   device out of the bootloader.
/// - **Raw.** The default line discipline echoes received bytes back to the
///   sender and rewrites the stream on the way through. See [`COMMAND_PREFIX`].
///
/// A plain file is not a tty; the terminal setup is skipped for one rather than
/// failing, because the tests exercise the framing over ordinary files.
fn open_raw(port: &Path) -> Result<std::fs::File, String> {
    use std::os::fd::FromRawFd;

    let c_path = std::ffi::CString::new(port.as_os_str().as_encoded_bytes())
        .map_err(|_| format!("can't open {}: path contains a NUL", port.display()))?;

    // SAFETY: `c_path` outlives the call and is NUL-terminated. The descriptor is
    // handed straight to `File`, which takes ownership and closes it.
    let fd = unsafe {
        libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK)
    };
    if fd < 0 {
        return Err(format!("can't open {}: {}", port.display(), std::io::Error::last_os_error()));
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    make_raw(&file);
    Ok(file)
}

/// Turn off echo, canonical input, and output translation, if this is a tty.
fn make_raw(file: &std::fs::File) {
    use std::os::fd::AsRawFd;

    let fd = file.as_raw_fd();
    // SAFETY: `fd` is owned by `file` for the whole call, and `tcgetattr` fully
    // initialises `tio` before anything reads it.
    unsafe {
        let mut tio: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut tio) != 0 {
            return; // not a tty — a plain file in tests
        }
        libc::cfmakeraw(&mut tio);
        tio.c_cc[libc::VMIN] = 0;
        tio.c_cc[libc::VTIME] = 0;
        libc::tcsetattr(fd, libc::TCSANOW, &tio);
    }
}

/// Write every byte, waiting out the short stalls a non-blocking tty can report.
fn write_all_raw(file: &mut std::fs::File, mut buf: &[u8]) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
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

/// Send `mode` to the device on `port`.
///
/// The command goes out as [`COMMAND_PREFIX`] then the mode byte. A trailing
/// newline follows so the same thing can be typed into a terminal; the firmware
/// ignores it.
pub fn send_to(port: &Path, mode: Mode) -> Result<(), String> {
    let mut file = open_raw(port)?;
    write_all_raw(&mut file, &[COMMAND_PREFIX, mode.command(), b'\n'])
        .map_err(|e| format!("can't write to {}: {e}", port.display()))?;
    Ok(())
}

/// The banner the firmware sends in reply to `?`. Must match `IDENTITY` in
/// `firmware/src/main.rs`.
pub const FIRMWARE_BANNER: &str = "openmicro-fw";

/// How long to wait for a reply to `?`. The firmware answers immediately; the
/// ROM bootloader answers never, so this is purely how long "never" takes.
pub const IDENTIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(600);

/// Does OpenMicro firmware answer on `port`?
///
/// This exists because USB id `303a:1001` is **ambiguous**: it is both the
/// ESP32-S3 ROM bootloader and *any* firmware exposing a USB-Serial-JTAG console
/// — which ours does, for its logs. So the id alone cannot distinguish "running
/// our firmware" from "sitting in the bootloader", and reading it as the latter
/// is why the TUI used to insist the device was in bootloader mode forever.
///
/// The distinguishing fact is behavioural: our firmware replies to `?`, and the
/// ROM bootloader does not.
pub fn firmware_answers_on(port: &Path) -> bool {
    let Ok(mut file) = open_raw(port) else {
        return false;
    };
    if write_all_raw(&mut file, &[COMMAND_PREFIX, b'?', b'\n']).is_err() {
        return false;
    }

    // Read in slices rather than to end-of-file: this is a character device, so
    // there is no EOF, and the firmware is also emitting its own periodic log
    // lines that we have to read past.
    //
    // The port is non-blocking, so `WouldBlock` is the normal "nothing yet"
    // answer and the deadline below is what actually bounds this. With a
    // blocking descriptor a silent device never returns from the first read at
    // all, and no deadline written here can help.
    let deadline = std::time::Instant::now() + IDENTIFY_TIMEOUT;
    let mut seen = String::new();
    let mut buf = [0u8; 256];
    while std::time::Instant::now() < deadline {
        match file.read(&mut buf) {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Ok(n) => {
                seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                if seen.contains(FIRMWARE_BANNER) {
                    return true;
                }
                // Bound the buffer: a chatty device must not grow this forever.
                if seen.len() > 8192 {
                    seen.clear();
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return false,
        }
    }
    false
}

/// Is our firmware running, on whichever port the device is on?
pub fn firmware_answers() -> bool {
    find_port().is_some_and(|p| firmware_answers_on(&p))
}

/// Send `mode` to whichever port the device is on.
///
/// The daemon is stood down for the write and started again afterwards: it holds
/// the same port, and two readers on one character device take a share of the
/// bytes each. Demo and identify keep running across the restart because the
/// firmware ignores host frames in those modes.
pub fn send(mode: Mode) -> Result<Vec<String>, String> {
    let port = find_port().ok_or_else(|| {
        "No serial port found. Is the device plugged in with a cable?".to_string()
    })?;
    let (_, mut log) = crate::daemon::with_paused(|| send_to(&port, mode))?;
    log.push(format!("{} mode, via {}", mode.label(), port.display()));
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_matches_the_firmware() {
        // Contract with `IDENTITY` in firmware/src/main.rs. If these drift, the
        // TUI silently reports every running device as being in bootloader mode.
        assert_eq!(FIRMWARE_BANNER, "openmicro-fw");
    }

    #[test]
    fn identifying_a_missing_port_is_false_not_a_panic() {
        assert!(!firmware_answers_on(Path::new("/definitely/not/a/port")));
    }

    #[test]
    fn a_silent_port_does_not_identify_as_our_firmware() {
        // The ROM bootloader is exactly this: it accepts the write and says
        // nothing. It must not be mistaken for running firmware.
        let dir = std::env::temp_dir().join(format!("om-ident-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("silent");
        std::fs::write(&path, b"").unwrap();
        assert!(!firmware_answers_on(&path), "a silent device must not identify");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_silent_tty_like_port_times_out_instead_of_hanging() {
        // The regression test for the wizard hanging at "Starting the new
        // firmware". A plain file cannot model a quiet device — it reports EOF
        // straight away — so the original bug slipped through. A FIFO with no
        // writer behaves the way a silent tty does: reads never complete. Held
        // with a *blocking* descriptor this test does not fail, it hangs, which
        // is exactly what the wizard did.
        //
        // One wrinkle worth keeping: a FIFO opened `O_RDWR` loops back, so the
        // first read returns the probe's own `!?` and it is the *second* that
        // blocks. Verified by hand. Do not "simplify" this into a single read.
        let dir = std::env::temp_dir().join(format!("om-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quiet");
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path in a directory we just created.
        let made = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(made, 0, "could not create a FIFO to test against");

        let started = std::time::Instant::now();
        let answered = firmware_answers_on(&path);
        let took = started.elapsed();

        assert!(!answered, "a silent device must not identify as our firmware");
        assert!(
            took < std::time::Duration::from_secs(5),
            "the probe took {took:?}; it is blocking rather than timing out"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_prefix_is_a_byte_the_firmware_never_logs() {
        // The whole point of the prefix is that the firmware's own output,
        // echoed back by the tty, cannot look like a command. A letter, digit or
        // whitespace byte would appear in log lines constantly.
        assert!(!COMMAND_PREFIX.is_ascii_alphanumeric(), "prefix appears in ordinary words");
        assert!(!COMMAND_PREFIX.is_ascii_whitespace(), "prefix appears in every line");
    }

    #[test]
    fn a_port_already_carrying_the_banner_identifies() {
        // Stands in for the firmware's reply. Also checks the banner is found
        // amid other log output rather than needing to be the whole content.
        let dir = std::env::temp_dir().join(format!("om-ident2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chatty");
        std::fs::write(&path, format!("alive t=1\n{FIRMWARE_BANNER} 0.3.0\nalive t=2\n")).unwrap();
        assert!(firmware_answers_on(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_bytes_match_the_firmware() {
        // These three bytes are a contract with `handle_serial_command` in the
        // firmware. Changing one here without changing it there would leave the
        // menu silently doing nothing.
        assert_eq!(Mode::Normal.command(), b'n');
        assert_eq!(Mode::Demo.command(), b'd');
        assert_eq!(Mode::Identify.command(), b'i');
    }

    #[test]
    fn every_mode_has_a_distinct_command() {
        let modes = [Mode::Normal, Mode::Demo, Mode::Identify];
        for (i, a) in modes.iter().enumerate() {
            for b in modes.iter().skip(i + 1) {
                assert_ne!(a.command(), b.command(), "{a:?} and {b:?} share a byte");
            }
        }
    }

    #[test]
    fn commands_are_never_whitespace() {
        // The firmware skips whitespace so a line-buffered terminal's newline is
        // not read as an unknown command — a command byte that *was* whitespace
        // would therefore be silently dropped.
        for mode in [Mode::Normal, Mode::Demo, Mode::Identify] {
            assert!(!mode.command().is_ascii_whitespace(), "{mode:?} is whitespace");
        }
    }

    #[test]
    fn writing_sends_the_command_byte_and_a_newline() {
        let dir = std::env::temp_dir().join(format!("om-display-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-port");
        std::fs::write(&path, b"").unwrap();

        send_to(&path, Mode::Demo).expect("writing to a plain file should work");
        assert_eq!(std::fs::read(&path).unwrap(), b"!d\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_port_is_an_error_not_a_panic() {
        let err = send_to(Path::new("/definitely/not/a/port"), Mode::Normal)
            .expect_err("a missing port must fail");
        assert!(err.contains("can't open"), "unhelpful error: {err}");
    }

    #[test]
    fn candidates_are_acm_ports_in_order() {
        let ports = candidate_ports();
        assert_eq!(ports[0], PathBuf::from("/dev/ttyACM0"));
        assert!(ports.len() > 1, "one candidate is not enough to find a moved device");
        for p in &ports {
            assert!(p.to_string_lossy().starts_with("/dev/ttyACM"));
        }
    }
}
