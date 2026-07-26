use openmicro_proto::AgentColors;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// No device: render into memory. Useful for testing the daemon alone.
    Mock,
    /// USB cable, over the firmware's USB-Serial-JTAG console.
    ///
    /// The default, and the only transport that currently reaches real hardware:
    /// the firmware's BLE GATT server is still a sketch, so `Ble` cannot deliver
    /// a frame. Cable also needs no pairing and cannot drop out mid-session.
    #[default]
    Cable,
    /// Bluetooth LE. Requires a firmware GATT server, which does not exist yet.
    Ble,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub brightness: u8,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub colors: AgentColors,
    #[serde(default = "default_sleep_minutes")]
    pub sleep_minutes: u32,
}

fn default_sleep_minutes() -> u32 {
    3
}

impl Default for Config {
    fn default() -> Self {
        Self {
            brightness: 200,
            transport: Transport::Cable,
            colors: AgentColors::default(),
            sleep_minutes: default_sleep_minutes(),
        }
    }
}

static SAVE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Config {
    pub fn from_toml_str(s: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(s)
    }

    /// Serialize to TOML and write atomically: write to a temp file in the same
    /// directory, then rename over the target (rename is atomic on the same fs).
    pub fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = toml::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension(format!(
            "toml.tmp.{}.{}",
            std::process::id(),
            SAVE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(body.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

use std::path::PathBuf;

pub fn default_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/openmicro/config.toml")
}

pub fn load() -> Config {
    load_existing().unwrap_or_else(|e| {
        eprintln!("openmicrod: {e}; using defaults");
        Config::default()
    })
}

pub fn load_existing() -> Result<Config, String> {
    load_existing_from(&default_path())
}

pub fn load_existing_from(path: &std::path::Path) -> Result<Config, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Config::from_toml_str(&s)
            .map_err(|e| format!("config file {} is invalid: {e}", path.display())),
        Err(_) => Ok(Config::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        assert_eq!(Config::from_toml_str("").unwrap().brightness, 200);
    }

    #[test]
    fn reads_brightness() {
        assert_eq!(Config::from_toml_str("brightness = 80").unwrap().brightness, 80);
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(Config::from_toml_str("brightness = \"nope\"").is_err());
        assert!(Config::from_toml_str("brightness = ").is_err());
    }

    #[test]
    fn transport_ble_parses() {
        assert_eq!(Config::from_toml_str("transport = \"ble\"").unwrap().transport, Transport::Ble);
    }

    #[test]
    fn transport_defaults_to_cable() {
        // Cable, not BLE: it is the only transport that currently reaches real
        // hardware, and it needs no pairing.
        assert_eq!(Config::from_toml_str("").unwrap().transport, Transport::Cable);
        assert_eq!(Config::from_toml_str("brightness = 80").unwrap().transport, Transport::Cable);
    }

    #[test]
    fn transport_cable_and_mock_parse() {
        assert_eq!(
            Config::from_toml_str("transport = \"cable\"").unwrap().transport,
            Transport::Cable
        );
        assert_eq!(
            Config::from_toml_str("transport = \"mock\"").unwrap().transport,
            Transport::Mock
        );
    }

    #[test]
    fn config_with_colors_roundtrips() {
        use openmicro_proto::{AgentColors, Rgb};
        let cfg = Config {
            brightness: 77,
            colors: AgentColors { claude: Rgb { r: 1, g: 2, b: 3 }, ..Default::default() },
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let back = Config::from_toml_str(&toml).unwrap();
        assert_eq!(back.brightness, 77);
        assert_eq!(back.colors.claude, Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(back.colors.codex, cfg.colors.codex, "untouched agents keep their colour");
    }

    #[test]
    fn a_config_from_an_older_build_still_loads() {
        // Before colour meant "which agent", this table held per-state keys. A
        // user upgrading must not lose their brightness because of it.
        let old = "brightness = 42\n\n[colors]\nidle = { r = 0, g = 0, b = 0 }\n\
                   working = { r = 0, g = 200, b = 80 }\n";
        let back = Config::from_toml_str(old).unwrap();
        assert_eq!(back.brightness, 42, "brightness survived the stale colour table");
        assert_eq!(back.colors, openmicro_proto::AgentColors::default());
    }

    #[test]
    fn save_to_writes_file() {
        use openmicro_proto::{AgentColors, Rgb};
        let dir = std::env::temp_dir().join(format!("omtest-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = Config {
            brightness: 55,
            colors: AgentColors { grok: Rgb { r: 9, g: 8, b: 7 }, ..Default::default() },
            ..Default::default()
        };
        cfg.save_to(&path).unwrap();
        let back = Config::from_toml_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.brightness, 55);
        assert_eq!(back.colors.grok, Rgb { r: 9, g: 8, b: 7 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_to_leaves_no_temp_file_behind() {
        let dir = std::env::temp_dir().join(format!("omtest-tmp-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        Config::default().save_to(&path).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|name| name != "config.toml")
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_saves_leave_a_parseable_file() {
        let dir = std::env::temp_dir().join(format!("omtest-race-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let threads: Vec<_> = (0..8u8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let cfg = Config { brightness: 100 + i, ..Default::default() };
                    for _ in 0..20 {
                        cfg.save_to(&path).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }

        let back = Config::from_toml_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!((100..108).contains(&back.brightness), "unexpected winner: {}", back.brightness);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sleep_minutes_defaults_to_three() {
        assert_eq!(Config::from_toml_str("").unwrap().sleep_minutes, 3);
        assert_eq!(Config::from_toml_str("brightness = 80").unwrap().sleep_minutes, 3);
    }

    #[test]
    fn sleep_minutes_parses_and_roundtrips() {
        assert_eq!(Config::from_toml_str("sleep_minutes = 10").unwrap().sleep_minutes, 10);
        let cfg = Config { sleep_minutes: 7, ..Default::default() };
        let back = Config::from_toml_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.sleep_minutes, 7);
    }

    #[test]
    fn invalid_transport_is_an_error() {
        assert!(Config::from_toml_str("transport = \"laser\"").is_err());
    }

    #[test]
    fn load_existing_from_missing_file_is_the_default() {
        let path = std::env::temp_dir().join("omtest-definitely-missing/config.toml");
        let cfg = load_existing_from(&path).unwrap();
        assert_eq!(cfg.brightness, 200);
    }

    #[test]
    fn load_existing_from_a_broken_file_is_an_error() {
        let dir = std::env::temp_dir().join(format!("omtest-broken-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "brightness = \"nope\"").unwrap();
        let err = load_existing_from(&path).unwrap_err();
        assert!(err.contains("invalid"), "unhelpful error: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
