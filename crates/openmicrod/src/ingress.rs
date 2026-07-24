use openmicro_proto::AgentState;
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
pub struct HookEvent {
    pub agent: String,
    pub session: String,
    pub state: String,
}

pub fn parse_state(s: &str) -> Option<AgentState> {
    match s {
        "idle" => Some(AgentState::Idle),
        "thinking" => Some(AgentState::Thinking),
        "working" => Some(AgentState::Working),
        "awaiting_approval" => Some(AgentState::AwaitingApproval),
        _ => None,
    }
}

pub fn parse_line(line: &str) -> Option<(String, String, AgentState)> {
    let ev: HookEvent = serde_json::from_str(line.trim()).ok()?;
    let state = parse_state(&ev.state)?;
    Some((ev.agent, ev.session, state))
}

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use crate::device::DeviceLink;
use crate::engine::Engine;

pub async fn serve(
    path: std::path::PathBuf,
    engine: Arc<Mutex<Engine>>,
    device: Arc<Mutex<dyn DeviceLink + Send>>,
) -> anyhow::Result<()> {
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    loop {
        let (stream, _) = listener.accept().await?;
        let engine = engine.clone();
        let device = device.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some((agent, session, state)) = parse_line(&line) {
                    let mut eng = engine.lock().await;
                    let mut dev = device.lock().await;
                    eng.on_event(&agent, &session, state, &mut *dev);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_event() {
        let line = r#"{"agent":"claude","session":"s1","state":"working"}"#;
        let (a, s, st) = parse_line(line).unwrap();
        assert_eq!(a, "claude");
        assert_eq!(s, "s1");
        assert_eq!(st, AgentState::Working);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_line("not json").is_none());
        assert!(parse_line(r#"{"agent":"x","session":"y","state":"bogus"}"#).is_none());
    }
}
