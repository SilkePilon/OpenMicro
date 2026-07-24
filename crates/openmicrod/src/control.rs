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
    /// Battery percentage (0..=100) when known, else None (e.g. mock transport).
    pub battery: Option<u8>,
    /// Whether the device reports charging. Often unknown over plain BLE
    /// Battery Service, in which case it is false.
    pub charging: bool,
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

pub fn snapshot(engine: &Engine, battery: Option<openmicro_proto::Battery>) -> Snapshot {
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
    Snapshot {
        sessions,
        owner,
        battery: battery.map(|b| b.pct),
        charging: battery.map(|b| b.charging).unwrap_or(false),
    }
}

/// Persist the given brightness + colors to config, preserving the on-disk
/// transport (and any other) fields by loading the current config first.
///
/// This performs blocking synchronous filesystem I/O (read + write + rename),
/// so it must NOT be called while any engine/device lock is held. Callers
/// capture the fields to persist, drop their guards, and then invoke this.
/// Best-effort: errors are logged, not propagated.
fn persist(brightness: u8, colors: openmicro_proto::StateColors, sleep_minutes: u32) {
    let mut cfg = crate::config::load();
    cfg.brightness = brightness;
    cfg.colors = colors;
    cfg.sleep_minutes = sleep_minutes;
    if let Err(e) = cfg.save() {
        eprintln!("openmicrod: failed to persist config: {e}");
    }
}

pub async fn serve(
    path: std::path::PathBuf,
    engine: Arc<Mutex<Engine>>,
    device: Arc<Mutex<dyn DeviceLink + Send>>,
    battery: Arc<Mutex<Option<openmicro_proto::Battery>>>,
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
        let battery_w = battery.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let snap = {
                    let bat = *battery_w.lock().await;
                    let eng = engine_w.lock().await;
                    snapshot(&eng, bat)
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
                // Lock order: engine -> device (mirrors ingress). Apply the
                // command under the locks, capture the fields to persist, then
                // drop both guards BEFORE doing any blocking filesystem I/O so
                // other tasks (snapshot writer, ingress hook events) aren't
                // stalled waiting on the engine lock.
                let (brightness, colors, sleep_minutes) = {
                    let mut eng = engine_r.lock().await;
                    let mut dev = device_r.lock().await;
                    eng.apply_command(cmd, &mut *dev).await;
                    let fields = eng.to_config_fields();
                    drop(dev);
                    drop(eng);
                    fields
                };
                // No engine/device lock is held here: run the sync read/write
                // /rename off the async runtime so it can't block other tasks.
                let _ = tokio::task::spawn_blocking(move || {
                    persist(brightness, colors, sleep_minutes)
                })
                .await;
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
        let snap = snapshot(&engine, None);
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.sessions[0].agent, "claude");
        assert_eq!(snap.sessions[0].state, "awaiting_approval");
        assert_eq!(snap.owner.as_deref(), Some("claude:s1"));
    }

    #[tokio::test]
    async fn snapshot_includes_battery_when_present() {
        let engine = Engine::new(255);
        let snap = snapshot(&engine, Some(openmicro_proto::Battery { pct: 84, charging: true }));
        assert_eq!(snap.battery, Some(84));
        assert!(snap.charging);
    }

    #[tokio::test]
    async fn snapshot_battery_none_when_unset() {
        let engine = Engine::new(255);
        let snap = snapshot(&engine, None);
        assert_eq!(snap.battery, None);
        assert!(!snap.charging);
    }
}
