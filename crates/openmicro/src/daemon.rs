use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

pub const UNIT: &str = "openmicrod.service";

pub const UNIT_REL: &str = ".config/systemd/user/openmicrod.service";

const START_TIMEOUT: Duration = Duration::from_secs(5);

const STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub fn socket_path() -> PathBuf {
    openmicro_proto::paths::control_socket()
}

pub fn unit_path() -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir).join("systemd/user").join(UNIT),
        _ => crate::agents::home().join(UNIT_REL),
    }
}

pub fn is_socket_live(path: &std::path::Path) -> bool {
    UnixStream::connect(path).is_ok()
}

pub fn is_running() -> bool {
    is_socket_live(&socket_path())
}

fn missing_unit_error(unit: &std::path::Path) -> String {
    format!(
        "no systemd user unit at {}. Run packaging/install.sh from an OpenMicro \
         checkout to install the service.",
        unit.display()
    )
}

pub fn unit_installed() -> bool {
    unit_path().is_file()
}

pub fn have_systemctl() -> bool {
    crate::flash::which(&["systemctl"]).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub running: bool,
    pub unit_installed: bool,
    pub enabled: bool,
}

impl Status {
    pub fn describe(&self) -> String {
        match (self.running, self.unit_installed, self.enabled) {
            (true, _, true) => "running, starts on login".to_string(),
            (true, _, false) => "running, but not enabled at login".to_string(),
            (false, true, _) => "installed but not running".to_string(),
            (false, false, _) => "not running (no service installed)".to_string(),
        }
    }
}

pub fn status() -> Status {
    Status {
        running: is_running(),
        unit_installed: unit_installed(),
        enabled: is_enabled(),
    }
}

fn is_enabled() -> bool {
    if !have_systemctl() {
        return false;
    }
    Command::new("systemctl")
        .args(["--user", "is-enabled", UNIT])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn systemctl(verb: &str) -> Result<Vec<String>, String> {
    if !have_systemctl() {
        return Err(
            "systemctl not found — this system does not use systemd, so the daemon has to \
             be started by hand (run `openmicrod`)."
                .to_string(),
        );
    }
    let output = Command::new("systemctl")
        .args(["--user", verb, UNIT])
        .output()
        .map_err(|e| format!("cannot run systemctl: {e}"))?;
    let lines = crate::flash::combine_output(&output.stdout, &output.stderr);
    if output.status.success() {
        Ok(lines)
    } else {
        Err(if lines.is_empty() {
            format!("systemctl {verb} failed")
        } else {
            lines.join("\n")
        })
    }
}

pub fn start() -> Result<Vec<String>, String> {
    if is_running() {
        return Ok(vec!["daemon is already running.".to_string()]);
    }
    if !unit_installed() {
        return Err(missing_unit_error(&unit_path()));
    }
    let mut log = systemctl("start")?;
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if is_running() {
            log.push("daemon is running.".to_string());
            return Ok(log);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err(format!(
        "systemd started the unit but nothing is listening on {} after {}s. \
         Check `systemctl --user status {UNIT}`.",
        socket_path().display(),
        START_TIMEOUT.as_secs()
    ))
}

pub fn stop() -> Result<Vec<String>, String> {
    if !is_running() && !unit_installed() {
        return Ok(vec!["daemon is not running.".to_string()]);
    }
    systemctl("stop")
}

pub fn enable() -> Result<Vec<String>, String> {
    if !unit_installed() {
        return Err(format!("no systemd user unit at {}.", unit_path().display()));
    }
    let mut log = systemctl("enable")?;
    log.extend(start()?);
    Ok(log)
}

pub fn disable() -> Result<Vec<String>, String> {
    systemctl("disable")
}

pub fn with_paused<T>(job: impl FnOnce() -> Result<T, String>) -> Result<(T, Vec<String>), String> {
    if !is_running() {
        return job().map(|value| (value, Vec::new()));
    }
    if !unit_installed() {
        return Err(format!(
            "the background service is running but was not started by systemd, so it \
             cannot be paused to free the serial port. Stop it by hand, then retry \
             (no unit at {}).",
            unit_path().display()
        ));
    }

    let mut log = vec!["paused the background service".to_string()];
    stop()?;
    wait_until_stopped();

    let result = job();

    match start() {
        Ok(_) => log.push("background service running again".to_string()),
        Err(e) => log.push(format!("could not restart the background service: {e}")),
    }
    result.map(|value| (value, log))
}

fn wait_until_stopped() {
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !is_running() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn restart() -> Result<Vec<String>, String> {
    let mut log = systemctl("restart")?;
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if is_running() {
            log.push("daemon is running.".to_string());
            return Ok(log);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err("the daemon did not come back after a restart.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_socket_lives_under_the_runtime_dir() {
        let ctl = socket_path();
        assert!(ctl.ends_with("openmicro-ctl.sock"), "{ctl:?}");
        assert!(ctl.parent().is_some());
    }

    #[test]
    fn unit_path_ends_at_the_user_unit() {
        assert!(unit_path().ends_with(UNIT), "{:?}", unit_path());
        assert!(unit_path().to_string_lossy().contains("systemd/user"));
    }

    #[test]
    fn running_is_decided_by_connecting_not_by_a_leftover_socket_file() {
        let dir = std::env::temp_dir().join(format!("openmicro-daemon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("openmicro-ctl.sock");
        std::fs::write(&stale, b"stale").unwrap();

        assert!(!is_socket_live(&stale), "a stale socket file is not a live daemon");
        assert!(!is_socket_live(&dir.join("nothing-here.sock")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_listener_reads_as_running() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("openmicro-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("openmicro-ctl.sock");
        let _listener = UnixListener::bind(&path).unwrap();

        assert!(is_socket_live(&path));

        drop(_listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_descriptions_cover_every_combination() {
        let cases = [
            (true, true, true, "starts on login"),
            (true, true, false, "not enabled"),
            (false, true, false, "not running"),
            (false, false, false, "no service installed"),
        ];
        for (running, unit_installed, enabled, expected) in cases {
            let s = Status { running, unit_installed, enabled };
            assert!(
                s.describe().contains(expected),
                "{s:?} described as {:?}, expected it to mention {expected:?}",
                s.describe()
            );
        }
    }

    #[test]
    fn starting_without_a_unit_explains_how_to_install_one() {
        let err = missing_unit_error(std::path::Path::new("/home/u/.config/systemd/user/x.service"));
        assert!(err.contains("install.sh"), "{err}");
        assert!(err.contains("/home/u/.config/systemd/user/x.service"), "{err}");
    }
}
