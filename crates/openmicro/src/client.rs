use openmicro_proto::Rgb;
use serde::Deserialize;

/// Small preset palette the config panel cycles through for per-state colors.
pub const PALETTE: [Rgb; 6] = [
    Rgb { r: 255, g: 255, b: 255 },
    Rgb { r: 255, g: 0, b: 0 },
    Rgb { r: 0, g: 200, b: 80 },
    Rgb { r: 40, g: 90, b: 255 },
    Rgb { r: 255, g: 140, b: 0 },
    Rgb { r: 160, g: 60, b: 255 },
];

/// Step a palette index by `dir` (+1/-1), wrapping within `PALETTE`.
pub fn step_preset(idx: usize, dir: i32) -> usize {
    let len = PALETTE.len() as i32;
    (((idx as i32 + dir) % len + len) % len) as usize
}

/// Adjust brightness by `delta`, clamped to `0..=255`.
pub fn adjust_brightness(b: u8, delta: i32) -> u8 {
    (b as i32 + delta).clamp(0, 255) as u8
}

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
    /// Live LED brightness from the daemon. `#[serde(default)]` so a snapshot
    /// from an older daemon build (before Fix 1) still parses; falls back to
    /// the historical UI default (200) rather than 0.
    #[serde(default = "default_brightness")]
    pub brightness: u8,
    /// Live per-state LED colors from the daemon, same default rationale.
    #[serde(default)]
    pub colors: openmicro_proto::StateColors,
    /// Live idle-sleep minutes from the daemon, same default rationale.
    #[serde(default = "default_sleep_minutes")]
    pub sleep_minutes: u32,
}

/// Matches the historical hardcoded UI default (`ConfigUiState::new(200)`).
fn default_brightness() -> u8 {
    200
}

/// Matches the historical hardcoded UI default (`ConfigUiState`'s `sleep_minutes: 3`).
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
            colors: openmicro_proto::StateColors::default(),
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
        // Older daemon snapshots omit battery/charging entirely.
        let line = r#"{"sessions":[],"owner":null}"#;
        let snap = parse_snapshot(line).unwrap();
        assert_eq!(snap.battery, None);
        assert!(!snap.charging);
    }

    #[test]
    fn config_fields_default_when_absent() {
        // Fix 1: older daemon snapshots (or a stale build) omit brightness/
        // colors/sleep_minutes entirely; parsing must still succeed and fall
        // back to the current UI defaults rather than erroring.
        let line = r#"{"sessions":[],"owner":null}"#;
        let snap = parse_snapshot(line).unwrap();
        assert_eq!(snap.brightness, 200);
        assert_eq!(snap.colors, openmicro_proto::StateColors::default());
        assert_eq!(snap.sleep_minutes, 3);
    }

    #[test]
    fn parses_config_fields_when_present() {
        // The daemon now carries the live config in every snapshot; the TUI
        // seeds its config panel from these instead of hardcoded constants.
        let line = r#"{"sessions":[],"owner":null,"brightness":77,
            "colors":{"idle":{"r":0,"g":0,"b":0},"thinking":{"r":40,"g":90,"b":255},
            "working":{"r":9,"g":8,"b":7},"awaiting_approval":{"r":255,"g":140,"b":0}},
            "sleep_minutes":42}"#;
        let snap = parse_snapshot(line).unwrap();
        assert_eq!(snap.brightness, 77);
        assert_eq!(snap.colors.working, openmicro_proto::Rgb { r: 9, g: 8, b: 7 });
        assert_eq!(snap.sleep_minutes, 42);
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse_snapshot("not json").is_none());
    }

    #[test]
    fn step_preset_wraps_both_directions() {
        assert_eq!(step_preset(0, 1), 1);
        assert_eq!(step_preset(PALETTE.len() - 1, 1), 0);
        assert_eq!(step_preset(0, -1), PALETTE.len() - 1);
    }

    #[test]
    fn adjust_brightness_clamps() {
        assert_eq!(adjust_brightness(100, 8), 108);
        assert_eq!(adjust_brightness(250, 8), 255);
        assert_eq!(adjust_brightness(4, -8), 0);
    }

    #[test]
    fn rejects_malformed_json() {
        // Valid JSON, but the wrong shape (`sessions` must be an array of
        // objects): deserialization fails and the `None` path is exercised.
        assert!(parse_snapshot(r#"{"sessions":"nope"}"#).is_none());
    }
}
