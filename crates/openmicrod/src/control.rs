use std::sync::Arc;

use openmicro_proto::Command;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use crate::device::DeviceLink;
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
    sessions.sort_by_key(|s| s.slot);
    Snapshot { sessions, owner }
}

/// Persist the engine's current brightness + colors to config, preserving the
/// on-disk transport setting. Best-effort: errors are logged, not propagated.
fn persist(engine: &Engine) {
    let (brightness, colors) = engine.to_config_fields();
    let mut cfg = crate::config::load();
    cfg.brightness = brightness;
    cfg.colors = colors;
    if let Err(e) = cfg.save() {
        eprintln!("openmicrod: failed to persist config: {e}");
    }
}

pub async fn serve(
    path: std::path::PathBuf,
    engine: Arc<Mutex<Engine>>,
    device: Arc<Mutex<dyn DeviceLink + Send>>,
) -> anyhow::Result<()> {
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("openmicrod: accept error on socket: {e}");
                continue;
            }
        };
        let (read_half, mut write_half) = stream.into_split();

        // Snapshot writer: 1/sec.
        let engine_w = engine.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let snap = {
                    let eng = engine_w.lock().await;
                    snapshot(&eng)
                };
                let mut line = serde_json::to_string(&snap).unwrap_or_default();
                line.push('\n');
                if write_half.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        // Command reader: newline-JSON `Command`s from the client.
        let engine_r = engine.clone();
        let device_r = device.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read_half).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let cmd: Command = match serde_json::from_str(line.trim()) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                // Lock order: engine -> device (mirrors ingress).
                let mut eng = engine_r.lock().await;
                let mut dev = device_r.lock().await;
                eng.apply_command(cmd, &mut *dev).await;
                drop(dev);
                persist(&eng);
            }
        });
    }
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
