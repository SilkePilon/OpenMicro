use openmicro_proto::Rgb;
use serde::Deserialize;

pub const PALETTE: [Rgb; 6] = [
    Rgb { r: 255, g: 255, b: 255 },
    Rgb { r: 255, g: 0, b: 0 },
    Rgb { r: 0, g: 200, b: 80 },
    Rgb { r: 40, g: 90, b: 255 },
    Rgb { r: 255, g: 140, b: 0 },
    Rgb { r: 160, g: 60, b: 255 },
];

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionDto {
    pub agent: String,
    pub session: String,
    pub state: String,
    pub slot: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotDto {
    pub sessions: Vec<SessionDto>,
    pub owner: Option<String>,
    #[serde(default)]
    pub battery: Option<u8>,
    #[serde(default)]
    pub charging: bool,
    #[serde(default = "default_brightness")]
    pub brightness: u8,
    #[serde(default)]
    pub colors: openmicro_proto::AgentColors,
    #[serde(default = "default_sleep_minutes")]
    pub sleep_minutes: u32,
}

fn default_brightness() -> u8 {
    200
}

fn default_sleep_minutes() -> u32 {
    3
}

impl Default for SnapshotDto {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            owner: None,
            battery: None,
            charging: false,
            brightness: default_brightness(),
            colors: openmicro_proto::AgentColors::default(),
            sleep_minutes: default_sleep_minutes(),
        }
    }
}

pub fn parse_snapshot(line: &str) -> Option<SnapshotDto> {
    serde_json::from_str(line.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_snapshot_line() {
        let line = r#"{"sessions":[{"agent":"claude","session":"s1","state":"working","slot":0}],"owner":"claude:s1"}"#;
        let snap = parse_snapshot(line).unwrap();
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.owner.as_deref(), Some("claude:s1"));
    }

    #[test]
    fn parses_battery_fields() {
        let line = r#"{"sessions":[],"owner":null,"battery":84,"charging":true}"#;
        let snap = parse_snapshot(line).unwrap();
        assert_eq!(snap.battery, Some(84));
        assert!(snap.charging);
    }

    #[test]
    fn battery_defaults_when_absent() {
        let line = r#"{"sessions":[],"owner":null}"#;
        let snap = parse_snapshot(line).unwrap();
        assert_eq!(snap.battery, None);
        assert!(!snap.charging);
    }

    #[test]
    fn config_fields_default_when_absent() {
        let line = r#"{"sessions":[],"owner":null}"#;
        let snap = parse_snapshot(line).unwrap();
        assert_eq!(snap.brightness, 200);
        assert_eq!(snap.colors, openmicro_proto::AgentColors::default());
        assert_eq!(snap.sleep_minutes, 3);
    }

    #[test]
    fn parses_config_fields_when_present() {
        let line = r#"{"sessions":[],"owner":null,"brightness":77,
            "colors":{"claude":{"r":9,"g":8,"b":7},"codex":{"r":230,"g":230,"b":230},
            "grok":{"r":160,"g":60,"b":255},"other":{"r":120,"g":120,"b":120}},
            "sleep_minutes":42}"#;
        let snap = parse_snapshot(line).unwrap();
        assert_eq!(snap.brightness, 77);
        assert_eq!(snap.colors.claude, openmicro_proto::Rgb { r: 9, g: 8, b: 7 });
        assert_eq!(snap.sleep_minutes, 42);
    }

    #[test]
    fn a_snapshot_from_an_older_daemon_still_parses() {
        let line = r#"{"sessions":[],"owner":null,"brightness":77,
            "colors":{"idle":{"r":0,"g":0,"b":0},"working":{"r":0,"g":200,"b":80}},
            "sleep_minutes":42}"#;
        let snap = parse_snapshot(line).expect("old snapshot shape must still parse");
        assert_eq!(snap.brightness, 77);
        assert_eq!(snap.colors, openmicro_proto::AgentColors::default());
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse_snapshot("not json").is_none());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_snapshot(r#"{"sessions":"nope"}"#).is_none());
    }
}
