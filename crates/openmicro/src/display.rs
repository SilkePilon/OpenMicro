//! Switching the device's display mode from the TUI.
//!
//! # Why serial and not the daemon
//!
//! The obvious route would be a `Command` to the daemon, which would pass it to
//! the device. That does not work yet: the daemon reaches the device over BLE,
//! and the firmware's GATT server is still a sketch, so nothing the daemon sends
//! arrives. The firmware *does* expose USB-Serial-JTAG for its logs, and the RX
//! half of that is free — so a single byte down the same port switches modes on
//! an already-flashed device, with no BLE and no rebuild.
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

/// Send `mode` to the device on `port`.
///
/// Writing a raw byte to the character device is enough: USB-Serial-JTAG ignores
/// baud rate and the firmware reads one byte at a time, so there is no line
/// discipline to negotiate. A trailing newline is sent because the firmware
/// tolerates it and it makes the same command work if pasted into a terminal.
pub fn send_to(port: &Path, mode: Mode) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(port)
        .map_err(|e| format!("can't open {}: {e}", port.display()))?;
    file.write_all(&[mode.command(), b'\n'])
        .map_err(|e| format!("can't write to {}: {e}", port.display()))?;
    file.flush().map_err(|e| format!("can't flush {}: {e}", port.display()))?;
    Ok(())
}

/// The banner the firmware sends in reply to `?`. Must match `IDENTITY` in
/// `firmware/src/main.rs`.
pub const FIRMWARE_BANNER: &str = "openmicro-fw";

/// How long to wait for a reply to `?`. The firmware answers immediately; the
/// ROM bootloader answers never, so this is purely how long "never" takes.
const IDENTIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(600);

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
    let Ok(mut file) =
        std::fs::OpenOptions::new().read(true).write(true).open(port)
    else {
        return false;
    };
    if file.write_all(b"?\n").is_err() || file.flush().is_err() {
        return false;
    }

    // Read in slices rather than to end-of-file: this is a character device, so
    // there is no EOF, and the firmware is also emitting its own periodic log
    // lines that we have to read past.
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
pub fn send(mode: Mode) -> Result<String, String> {
    let port = find_port().ok_or_else(|| {
        "No serial port found. Is the device plugged in with a cable?".to_string()
    })?;
    send_to(&port, mode)?;
    Ok(format!("{} mode, via {}", mode.label(), port.display()))
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
        assert_eq!(std::fs::read(&path).unwrap(), b"d\n");

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
