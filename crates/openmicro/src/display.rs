use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Demo,
    Identify,
}

pub const COMMAND_PREFIX: u8 = b'!';

impl Mode {
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

fn candidate_ports() -> Vec<PathBuf> {
    (0..4).map(|i| PathBuf::from(format!("/dev/ttyACM{i}"))).collect()
}

pub fn find_port() -> Option<PathBuf> {
    candidate_ports().into_iter().find(|p| p.exists())
}

fn open_raw(port: &Path) -> Result<std::fs::File, String> {
    use std::os::fd::FromRawFd;

    let c_path = std::ffi::CString::new(port.as_os_str().as_encoded_bytes())
        .map_err(|_| format!("can't open {}: path contains a NUL", port.display()))?;

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

fn make_raw(file: &std::fs::File) {
    use std::os::fd::AsRawFd;

    let fd = file.as_raw_fd();
    unsafe {
        let mut tio: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut tio) != 0 {
            return;
        }
        libc::cfmakeraw(&mut tio);
        tio.c_cc[libc::VMIN] = 0;
        tio.c_cc[libc::VTIME] = 0;
        libc::tcsetattr(fd, libc::TCSANOW, &tio);
    }
}

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

pub fn send_to(port: &Path, mode: Mode) -> Result<(), String> {
    let mut file = open_raw(port)?;
    write_all_raw(&mut file, &[COMMAND_PREFIX, mode.command(), b'\n'])
        .map_err(|e| format!("can't write to {}: {e}", port.display()))?;
    Ok(())
}

pub const FIRMWARE_BANNER: &str = "openmicro-fw";

pub const IDENTIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(600);

pub fn firmware_answers_on(port: &Path) -> bool {
    let Ok(mut file) = open_raw(port) else {
        return false;
    };
    if write_all_raw(&mut file, &[COMMAND_PREFIX, b'?', b'\n']).is_err() {
        return false;
    }

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

pub fn firmware_answers() -> bool {
    find_port().is_some_and(|p| firmware_answers_on(&p))
}

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
        assert_eq!(FIRMWARE_BANNER, "openmicro-fw");
    }

    #[test]
    fn identifying_a_missing_port_is_false_not_a_panic() {
        assert!(!firmware_answers_on(Path::new("/definitely/not/a/port")));
    }

    #[test]
    fn a_silent_port_does_not_identify_as_our_firmware() {
        let dir = std::env::temp_dir().join(format!("om-ident-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("silent");
        std::fs::write(&path, b"").unwrap();
        assert!(!firmware_answers_on(&path), "a silent device must not identify");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_silent_tty_like_port_times_out_instead_of_hanging() {
        let dir = std::env::temp_dir().join(format!("om-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quiet");
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
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
        assert!(!COMMAND_PREFIX.is_ascii_alphanumeric(), "prefix appears in ordinary words");
        assert!(!COMMAND_PREFIX.is_ascii_whitespace(), "prefix appears in every line");
    }

    #[test]
    fn a_port_already_carrying_the_banner_identifies() {
        let dir = std::env::temp_dir().join(format!("om-ident2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chatty");
        std::fs::write(&path, format!("alive t=1\n{FIRMWARE_BANNER} 0.3.0\nalive t=2\n")).unwrap();
        assert!(firmware_answers_on(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_bytes_match_the_firmware() {
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
