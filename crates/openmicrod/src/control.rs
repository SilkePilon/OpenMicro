use serde::Serialize;

use crate::engine::Engine;
use crate::focus::pick_owner;

#[derive(Debug, Serialize, PartialEq)]
pub struct SnapSession {
    pub agent: String,
    pub session: String,
    pub state: String,
    pub slot: Option<usize>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Snapshot {
    pub sessions: Vec<SnapSession>,
    pub owner: Option<String>,
}

fn state_name(state: openmicro_proto::AgentState) -> &'static str {
    use openmicro_proto::AgentState::*;
    match state {
        Idle => "idle",
        Thinking => "thinking",
        Working => "working",
        AwaitingApproval => "awaiting_approval",
    }
}

pub fn snapshot(engine: &Engine) -> Snapshot {
    let owner = pick_owner(engine.store.iter(), engine.pinned.as_ref())
        .map(|k| format!("{}:{}", k.agent, k.session));
    let mut sessions: Vec<SnapSession> = engine
        .store
        .iter()
        .map(|s| SnapSession {
            agent: s.key.agent.clone(),
            session: s.key.session.clone(),
            state: state_name(s.state).to_string(),
            slot: engine.mapping.slot_for(&s.key),
        })
        .collect();
    sessions.sort_by(|a, b| a.slot.cmp(&b.slot));
    Snapshot { sessions, owner }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MockDevice;
    use openmicro_proto::AgentState;

    #[tokio::test]
    async fn snapshot_reflects_engine() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::AwaitingApproval, &mut dev).await;
        let snap = snapshot(&engine);
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.sessions[0].agent, "claude");
        assert_eq!(snap.sessions[0].state, "awaiting_approval");
        assert_eq!(snap.owner.as_deref(), Some("claude:s1"));
    }
}

use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

pub async fn serve(path: std::path::PathBuf, engine: Arc<Mutex<Engine>>) -> anyhow::Result<()> {
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("openmicrod: accept error on socket: {e}");
                continue;
            }
        };
        let engine = engine.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let snap = {
                    let eng = engine.lock().await;
                    snapshot(&eng)
                };
                let mut line = serde_json::to_string(&snap).unwrap_or_default();
                line.push('\n');
                if stream.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
    }
}
