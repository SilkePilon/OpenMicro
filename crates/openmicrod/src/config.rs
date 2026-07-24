use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub brightness: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self { brightness: 200 }
    }
}

impl Config {
    pub fn from_toml_str(s: &str) -> Config {
        toml::from_str(s).unwrap_or_default()
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
}
