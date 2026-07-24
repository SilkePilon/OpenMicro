use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionDto {
    pub agent: String,
    pub session: String,
    pub state: String,
    pub slot: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SnapshotDto {
    pub sessions: Vec<SessionDto>,
    pub owner: Option<String>,
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
    fn rejects_non_json() {
        assert!(parse_snapshot("not json").is_none());
    }

    #[test]
    fn rejects_malformed_json() {
        // Valid JSON, but the wrong shape (`sessions` must be an array of
        // objects): deserialization fails and the `None` path is exercised.
        assert!(parse_snapshot(r#"{"sessions":"nope"}"#).is_none());
    }
}
