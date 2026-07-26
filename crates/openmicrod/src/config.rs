use openmicro_proto::AgentColors;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Mock,
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
            transport: Transport::Mock,
            colors: AgentColors::default(),
            sleep_minutes: default_sleep_minutes(),
        }
    }
}

impl Config {
    pub fn from_toml_str(s: &str) -> Config {
        toml::from_str(s).unwrap_or_default()
    }

    /// Serialize to TOML and write atomically: write to a temp file in the same
    /// directory, then rename over the target (rename is atomic on the same fs).
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&default_path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = toml::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, body.as_bytes())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

use std::path::PathBuf;

pub fn default_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/openmicro/config.toml")
}

pub fn load() -> Config {
    match std::fs::read_to_string(default_path()) {
        Ok(s) => Config::from_toml_str(&s),
        Err(_) => Config::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        assert_eq!(Config::from_toml_str("").brightness, 200);
    }

    #[test]
    fn reads_brightness() {
        assert_eq!(Config::from_toml_str("brightness = 80").brightness, 80);
    }

    #[test]
    fn invalid_falls_back_to_default() {
        assert_eq!(Config::from_toml_str("brightness = \"nope\"").brightness, 200);
    }

    #[test]
    fn transport_ble_parses() {
        assert_eq!(Config::from_toml_str("transport = \"ble\"").transport, Transport::Ble);
    }

    #[test]
    fn transport_defaults_to_mock() {
        assert_eq!(Config::from_toml_str("").transport, Transport::Mock);
        assert_eq!(Config::from_toml_str("brightness = 80").transport, Transport::Mock);
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
        let back = Config::from_toml_str(&toml);
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
        let back = Config::from_toml_str(old);
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
        let back = Config::from_toml_str(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(back.brightness, 55);
        assert_eq!(back.colors.grok, Rgb { r: 9, g: 8, b: 7 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sleep_minutes_defaults_to_three() {
        assert_eq!(Config::from_toml_str("").sleep_minutes, 3);
        assert_eq!(Config::from_toml_str("brightness = 80").sleep_minutes, 3);
    }

    #[test]
    fn sleep_minutes_parses_and_roundtrips() {
        assert_eq!(Config::from_toml_str("sleep_minutes = 10").sleep_minutes, 10);
        let cfg = Config { sleep_minutes: 7, ..Default::default() };
        let back = Config::from_toml_str(&toml::to_string(&cfg).unwrap());
        assert_eq!(back.sleep_minutes, 7);
    }

    #[test]
    fn invalid_transport_falls_back_to_default_config() {
        let cfg = Config::from_toml_str("transport = \"laser\"");
        assert_eq!(cfg.transport, Transport::Mock);
        assert_eq!(cfg.brightness, 200);
    }
}
