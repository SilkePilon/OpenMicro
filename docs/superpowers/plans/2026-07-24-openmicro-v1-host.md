# OpenMicro v1 Host Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the hardware-free half of OpenMicro — the shared protocol crate, the daemon (state engine + focus policy + LED mapping + hook ingress + config + control socket) driving a MockDevice, the `openmicro-hook` shim, the Claude Code adapter, and an interactive TUI — all testable end-to-end with no device.

**Architecture:** A Cargo workspace. `openmicro-proto` defines the wire types shared with firmware. `openmicrod` is a tokio daemon: agent hooks push state over a unix socket → state engine + focus policy compute a `LedFrame` → a `DeviceLink` renders it (MockDevice for now, BLE later). A separate control socket streams snapshots to the `openmicro` ratatui TUI. `openmicro-hook` is the tiny CLI agent hooks call.

**Tech Stack:** Rust (stable), tokio, serde, postcard, toml, ratatui + crossterm, clap. All-Rust, no C deps for v1 host.

## Global Constraints

- Rust edition 2021, stable toolchain. No `unsafe` in the host crates.
- `openmicro-proto` is `#![no_std]` + `alloc`; gains `std` only behind a `std` feature. It must compile with `--no-default-features` for firmware reuse.
- Wire encoding between firmware and daemon is `postcard`. Hook-ingress and control sockets between host processes are newline-delimited JSON (human-debuggable).
- Config lives at `~/.config/openmicro/config.toml`. Runtime sockets at `$XDG_RUNTIME_DIR/openmicro.sock` (hook ingress) and `$XDG_RUNTIME_DIR/openmicro-ctl.sock` (TUI control).
- Agent-key slot count `N = 6` for v1 (const `SLOT_COUNT`); revisit at firmware pinout.
- Every task ends green (`cargo test` passing) and committed.

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/openmicro-proto/Cargo.toml`, `crates/openmicro-proto/src/lib.rs`
- Create: `crates/openmicrod/Cargo.toml`, `crates/openmicrod/src/main.rs`
- Create: `crates/openmicro/Cargo.toml`, `crates/openmicro/src/main.rs`
- Create: `crates/openmicro-hook/Cargo.toml`, `crates/openmicro-hook/src/main.rs`
- Create: `.gitignore`

**Interfaces:**
- Produces: a building 4-crate workspace.

- [ ] **Step 1: Write workspace `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = [
    "crates/openmicro-proto",
    "crates/openmicrod",
    "crates/openmicro",
    "crates/openmicro-hook",
]

[workspace.package]
edition = "2021"
license = "MIT"

[workspace.dependencies]
serde = { version = "1", default-features = false, features = ["derive"] }
postcard = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "io-util", "sync", "time", "fs"] }
serde_json = "1"
toml = "0.8"
anyhow = "1"
clap = { version = "4", features = ["derive"] }
ratatui = "0.28"
crossterm = "0.28"
```

- [ ] **Step 2: Write `.gitignore`**

```
/target
```

- [ ] **Step 3: Write each crate's `Cargo.toml`**

`crates/openmicro-proto/Cargo.toml`:
```toml
[package]
name = "openmicro-proto"
version = "0.1.0"
edition.workspace = true

[features]
default = []
std = ["serde/std", "postcard/use-std"]

[dependencies]
serde = { workspace = true }
postcard = { version = "1", default-features = false }
```

`crates/openmicrod/Cargo.toml`:
```toml
[package]
name = "openmicrod"
version = "0.1.0"
edition.workspace = true

[dependencies]
openmicro-proto = { path = "../openmicro-proto", features = ["std"] }
serde = { workspace = true, features = ["std"] }
serde_json.workspace = true
toml.workspace = true
tokio.workspace = true
anyhow.workspace = true
```

`crates/openmicro/Cargo.toml`:
```toml
[package]
name = "openmicro"
version = "0.1.0"
edition.workspace = true

[dependencies]
openmicro-proto = { path = "../openmicro-proto", features = ["std"] }
serde = { workspace = true, features = ["std"] }
serde_json.workspace = true
tokio.workspace = true
anyhow.workspace = true
ratatui.workspace = true
crossterm.workspace = true
clap.workspace = true
```

`crates/openmicro-hook/Cargo.toml`:
```toml
[package]
name = "openmicro-hook"
version = "0.1.0"
edition.workspace = true

[dependencies]
serde_json.workspace = true
clap.workspace = true
anyhow.workspace = true
```

- [ ] **Step 4: Write placeholder entrypoints**

`crates/openmicro-proto/src/lib.rs`:
```rust
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;
```

`crates/openmicrod/src/main.rs`, `crates/openmicro/src/main.rs`, `crates/openmicro-hook/src/main.rs` (each):
```rust
fn main() {}
```

- [ ] **Step 5: Verify the workspace builds**

Run: `cargo build`
Expected: compiles, no errors.

- [ ] **Step 6: Verify proto builds no_std**

Run: `cargo build -p openmicro-proto --no-default-features`
Expected: compiles (proves firmware reuse).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: scaffold OpenMicro Cargo workspace"
```

---

### Task 2: Protocol types + postcard roundtrip

**Files:**
- Create: `crates/openmicro-proto/src/types.rs`
- Modify: `crates/openmicro-proto/src/lib.rs`

**Interfaces:**
- Produces: `AgentState`, `Effect`, `Rgb`, `LedSlot`, `LedFrame`, `InputEvent`, `Battery`, `SLOT_COUNT`, and `LedFrame::{encode,decode}`.

- [ ] **Step 1: Write the failing test**

In `crates/openmicro-proto/src/types.rs`:
```rust
use serde::{Deserialize, Serialize};

pub const SLOT_COUNT: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    Idle,
    Thinking,
    Working,
    AwaitingApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Solid,
    Breath,
    Pulse,
    Rainbow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedSlot {
    pub color: Rgb,
    pub effect: Effect,
    pub brightness: u8,
}

impl LedSlot {
    pub const OFF: LedSlot = LedSlot {
        color: Rgb { r: 0, g: 0, b: 0 },
        effect: Effect::Solid,
        brightness: 0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedFrame {
    pub slots: [LedSlot; SLOT_COUNT],
}

impl LedFrame {
    pub const BLANK: LedFrame = LedFrame { slots: [LedSlot::OFF; SLOT_COUNT] };

    pub fn encode(&self) -> Result<heapless::Vec<u8, 256>, postcard::Error> {
        postcard::to_vec(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    Key { id: u8, pressed: bool },
    Encoder { delta: i8 },
    Joystick { dir: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub pct: u8,
    pub charging: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn led_frame_roundtrips() {
        let mut frame = LedFrame::BLANK;
        frame.slots[0] = LedSlot {
            color: Rgb { r: 10, g: 20, b: 30 },
            effect: Effect::Breath,
            brightness: 200,
        };
        let bytes = frame.encode().unwrap();
        let back = LedFrame::decode(&bytes).unwrap();
        assert_eq!(frame, back);
    }
}
```

- [ ] **Step 2: Add `heapless` dep and export the module**

In `crates/openmicro-proto/Cargo.toml` add under `[dependencies]`:
```toml
heapless = { version = "0.8", features = ["serde"] }
```

In `crates/openmicro-proto/src/lib.rs`:
```rust
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

mod types;
pub use types::*;
```

- [ ] **Step 3: Run test to verify it fails then passes**

Run: `cargo test -p openmicro-proto`
Expected: compiles and `led_frame_roundtrips` PASSES.

- [ ] **Step 4: Verify still no_std**

Run: `cargo build -p openmicro-proto --no-default-features`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(proto): agent/LED/input types with postcard roundtrip"
```

---

### Task 3: Session store + state engine

**Files:**
- Create: `crates/openmicrod/src/session.rs`
- Modify: `crates/openmicrod/src/main.rs` (add `mod session;`)

**Interfaces:**
- Consumes: `openmicro_proto::AgentState`.
- Produces: `SessionKey`, `Session`, `SessionStore` with
  `update(agent: &str, session: &str, state: AgentState)`,
  `get(&SessionKey) -> Option<&Session>`, `iter()`, `remove(&SessionKey)`.

- [ ] **Step 1: Write the failing test**

In `crates/openmicrod/src/session.rs`:
```rust
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use openmicro_proto::AgentState;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub agent: String,
    pub session: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub key: SessionKey,
    pub state: AgentState,
    pub updated_ms: u64,
}

#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: HashMap<SessionKey, Session>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, agent: &str, session: &str, state: AgentState) -> SessionKey {
        let key = SessionKey { agent: agent.to_string(), session: session.to_string() };
        let entry = self.sessions.entry(key.clone()).or_insert_with(|| Session {
            key: key.clone(),
            state,
            updated_ms: 0,
        });
        entry.state = state;
        entry.updated_ms = now_ms();
        key
    }

    pub fn get(&self, key: &SessionKey) -> Option<&Session> {
        self.sessions.get(key)
    }

    pub fn remove(&mut self, key: &SessionKey) {
        self.sessions.remove(key);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Session> {
        self.sessions.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_inserts_and_mutates() {
        let mut store = SessionStore::new();
        let k = store.update("claude", "abc", AgentState::Thinking);
        assert_eq!(store.get(&k).unwrap().state, AgentState::Thinking);
        store.update("claude", "abc", AgentState::Working);
        assert_eq!(store.get(&k).unwrap().state, AgentState::Working);
        assert_eq!(store.iter().count(), 1);
    }
}
```

- [ ] **Step 2: Register the module**

`crates/openmicrod/src/main.rs`:
```rust
mod session;

fn main() {}
```

- [ ] **Step 3: Run test**

Run: `cargo test -p openmicrod session`
Expected: `update_inserts_and_mutates` PASSES.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(daemon): session store + state engine"
```

---

### Task 4: Focus policy

**Files:**
- Create: `crates/openmicrod/src/focus.rs`
- Modify: `crates/openmicrod/src/main.rs` (add `mod focus;`)

**Interfaces:**
- Consumes: `session::{Session, SessionKey}`, `openmicro_proto::AgentState`.
- Produces: `fn pick_owner<'a>(sessions: impl Iterator<Item = &'a Session>, pinned: Option<&SessionKey>) -> Option<SessionKey>`.

- [ ] **Step 1: Write the failing test**

In `crates/openmicrod/src/focus.rs`:
```rust
use openmicro_proto::AgentState;

use crate::session::{Session, SessionKey};

/// Choose which session owns the deck. Rules, in order:
/// 1. If any session is AwaitingApproval, the most-recently-updated such one wins (preempt).
/// 2. Else if `pinned` names a live session, it wins.
/// 3. Else the most-recently-updated session wins.
pub fn pick_owner<'a>(
    sessions: impl Iterator<Item = &'a Session>,
    pinned: Option<&SessionKey>,
) -> Option<SessionKey> {
    let all: Vec<&Session> = sessions.collect();

    let mut awaiting: Vec<&Session> = all
        .iter()
        .copied()
        .filter(|s| s.state == AgentState::AwaitingApproval)
        .collect();
    if !awaiting.is_empty() {
        awaiting.sort_by_key(|s| s.updated_ms);
        return awaiting.last().map(|s| s.key.clone());
    }

    if let Some(p) = pinned {
        if all.iter().any(|s| &s.key == p) {
            return Some(p.clone());
        }
    }

    all.iter().max_by_key(|s| s.updated_ms).map(|s| s.key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionStore;

    fn store_with(entries: &[(&str, &str, AgentState)]) -> SessionStore {
        let mut store = SessionStore::new();
        for (a, s, st) in entries {
            store.update(a, s, *st);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        store
    }

    #[test]
    fn most_recent_wins_by_default() {
        let store = store_with(&[
            ("claude", "a", AgentState::Working),
            ("codex", "b", AgentState::Working),
        ]);
        let owner = pick_owner(store.iter(), None).unwrap();
        assert_eq!(owner.agent, "codex");
    }

    #[test]
    fn awaiting_approval_preempts() {
        let store = store_with(&[
            ("claude", "a", AgentState::AwaitingApproval),
            ("codex", "b", AgentState::Working),
        ]);
        let owner = pick_owner(store.iter(), None).unwrap();
        assert_eq!(owner.agent, "claude");
    }
}
```

- [ ] **Step 2: Register the module**

`crates/openmicrod/src/main.rs`:
```rust
mod focus;
mod session;

fn main() {}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p openmicrod focus`
Expected: both tests PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(daemon): focus policy with approval preemption"
```

---

### Task 5: State → LedFrame mapping

**Files:**
- Create: `crates/openmicrod/src/render.rs`
- Modify: `crates/openmicrod/src/main.rs` (add `mod render;`)

**Interfaces:**
- Consumes: `session::{Session, SessionKey}`, `openmicro_proto::{AgentState, Effect, LedFrame, LedSlot, Rgb, SLOT_COUNT}`.
- Produces: `fn state_color(state: AgentState) -> (Rgb, Effect)`,
  `fn render_frame(owner_slots: &[Option<AgentState>; SLOT_COUNT], brightness: u8) -> LedFrame`.

- [ ] **Step 1: Write the failing test**

In `crates/openmicrod/src/render.rs`:
```rust
use openmicro_proto::{AgentState, Effect, LedFrame, LedSlot, Rgb, SLOT_COUNT};

pub fn state_color(state: AgentState) -> (Rgb, Effect) {
    match state {
        AgentState::Idle => (Rgb { r: 0, g: 0, b: 0 }, Effect::Solid),
        AgentState::Thinking => (Rgb { r: 40, g: 90, b: 255 }, Effect::Breath),
        AgentState::Working => (Rgb { r: 0, g: 200, b: 80 }, Effect::Solid),
        AgentState::AwaitingApproval => (Rgb { r: 255, g: 140, b: 0 }, Effect::Pulse),
    }
}

/// Build a frame from per-slot assigned states. `None` = empty slot (off).
pub fn render_frame(slots: &[Option<AgentState>; SLOT_COUNT], brightness: u8) -> LedFrame {
    let mut frame = LedFrame::BLANK;
    for (i, maybe_state) in slots.iter().enumerate() {
        if let Some(state) = maybe_state {
            let (color, effect) = state_color(*state);
            frame.slots[i] = LedSlot { color, effect, brightness };
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigned_slot_gets_state_color() {
        let mut slots = [None; SLOT_COUNT];
        slots[2] = Some(AgentState::Working);
        let frame = render_frame(&slots, 128);
        assert_eq!(frame.slots[2].color, Rgb { r: 0, g: 200, b: 80 });
        assert_eq!(frame.slots[2].brightness, 128);
        assert_eq!(frame.slots[0], LedSlot::OFF);
    }
}
```

- [ ] **Step 2: Register the module**

Add `mod render;` to `crates/openmicrod/src/main.rs` (keep it alphabetical with the others).

- [ ] **Step 3: Run test**

Run: `cargo test -p openmicrod render`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(daemon): map agent state to LED frame"
```

---

### Task 6: DeviceLink trait + MockDevice + engine integration

**Files:**
- Create: `crates/openmicrod/src/device.rs`
- Create: `crates/openmicrod/src/engine.rs`
- Modify: `crates/openmicrod/src/main.rs` (add `mod device; mod engine;`)

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `trait DeviceLink { fn set_leds(&mut self, frame: &LedFrame); fn last_frame(&self) -> LedFrame; }`
  - `struct MockDevice` implementing it.
  - `struct Engine { store, mapping, pinned, brightness }` with
    `fn on_event(&mut self, agent: &str, session: &str, state: AgentState, device: &mut dyn DeviceLink)`
    and `fn slot_for(&self, key: &SessionKey) -> Option<usize>`.
  - `Mapping` = agent-name → slot index assignment, first-come-first-served across `SLOT_COUNT`.

- [ ] **Step 1: Write the failing integration test**

In `crates/openmicrod/src/device.rs`:
```rust
use openmicro_proto::LedFrame;

pub trait DeviceLink {
    fn set_leds(&mut self, frame: &LedFrame);
    fn last_frame(&self) -> LedFrame;
}

#[derive(Debug, Default)]
pub struct MockDevice {
    last: LedFrame,
    pub writes: usize,
}

impl MockDevice {
    pub fn new() -> Self {
        Self { last: LedFrame::BLANK, writes: 0 }
    }
}

impl DeviceLink for MockDevice {
    fn set_leds(&mut self, frame: &LedFrame) {
        self.last = *frame;
        self.writes += 1;
    }
    fn last_frame(&self) -> LedFrame {
        self.last
    }
}
```

In `crates/openmicrod/src/engine.rs`:
```rust
use std::collections::HashMap;

use openmicro_proto::{AgentState, SLOT_COUNT};

use crate::device::DeviceLink;
use crate::focus::pick_owner;
use crate::render::render_frame;
use crate::session::{SessionKey, SessionStore};

/// Assigns each session key a stable slot index, first-come-first-served.
#[derive(Debug, Default)]
pub struct Mapping {
    slots: HashMap<SessionKey, usize>,
}

impl Mapping {
    pub fn assign(&mut self, key: &SessionKey) -> Option<usize> {
        if let Some(i) = self.slots.get(key) {
            return Some(*i);
        }
        let used: std::collections::HashSet<usize> = self.slots.values().copied().collect();
        let free = (0..SLOT_COUNT).find(|i| !used.contains(i))?;
        self.slots.insert(key.clone(), free);
        Some(free)
    }
    pub fn slot_for(&self, key: &SessionKey) -> Option<usize> {
        self.slots.get(key).copied()
    }
}

pub struct Engine {
    pub store: SessionStore,
    pub mapping: Mapping,
    pub pinned: Option<SessionKey>,
    pub brightness: u8,
}

impl Engine {
    pub fn new(brightness: u8) -> Self {
        Self {
            store: SessionStore::new(),
            mapping: Mapping::default(),
            pinned: None,
            brightness,
        }
    }

    pub fn on_event(
        &mut self,
        agent: &str,
        session: &str,
        state: AgentState,
        device: &mut dyn DeviceLink,
    ) {
        let key = self.store.update(agent, session, state);
        self.mapping.assign(&key);
        self.rerender(device);
    }

    fn rerender(&self, device: &mut dyn DeviceLink) {
        let _owner = pick_owner(self.store.iter(), self.pinned.as_ref());
        let mut slots = [None; SLOT_COUNT];
        for session in self.store.iter() {
            if let Some(i) = self.mapping.slot_for(&session.key) {
                slots[i] = Some(session.state);
            }
        }
        let frame = render_frame(&slots, self.brightness);
        device.set_leds(&frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MockDevice;
    use openmicro_proto::Rgb;

    #[test]
    fn hook_event_lights_the_assigned_slot() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev);
        let frame = dev.last_frame();
        // claude:s1 got slot 0.
        assert_eq!(frame.slots[0].color, Rgb { r: 0, g: 200, b: 80 });
        assert_eq!(dev.writes, 1);
    }

    #[test]
    fn two_agents_get_distinct_slots() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev);
        engine.on_event("codex", "s2", AgentState::Thinking, &mut dev);
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0].color, Rgb { r: 0, g: 200, b: 80 }); // working
        assert_eq!(frame.slots[1].color, Rgb { r: 40, g: 90, b: 255 }); // thinking
    }
}
```

- [ ] **Step 2: Register modules**

`crates/openmicrod/src/main.rs`:
```rust
mod device;
mod engine;
mod focus;
mod render;
mod session;

fn main() {}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p openmicrod`
Expected: all engine tests PASS. This proves the whole host pipeline (event → state → mapping → frame) with no hardware.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(daemon): device link trait, mock device, engine integration"
```

---

### Task 7: Hook-ingress unix socket

**Files:**
- Create: `crates/openmicrod/src/ingress.rs`
- Modify: `crates/openmicrod/src/main.rs`

**Interfaces:**
- Consumes: `engine::Engine`, `device::DeviceLink`, `openmicro_proto::AgentState`.
- Produces: `fn parse_line(line: &str) -> Option<HookEvent>` where
  `HookEvent { agent: String, session: String, state: AgentState }`;
  `async fn serve(path, shared_engine, shared_device)`.

- [ ] **Step 1: Write the failing parse test**

In `crates/openmicrod/src/ingress.rs`:
```rust
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
```

- [ ] **Step 2: Add the async server function (same file)**

Append to `crates/openmicrod/src/ingress.rs`:
```rust
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
```

- [ ] **Step 3: Register module + add `dyn DeviceLink + Send` bound**

In `crates/openmicrod/src/device.rs`, change the trait to require `Send`:
```rust
pub trait DeviceLink: Send {
    fn set_leds(&mut self, frame: &LedFrame);
    fn last_frame(&self) -> LedFrame;
}
```
Add `mod ingress;` to `crates/openmicrod/src/main.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p openmicrod`
Expected: ingress parse tests PASS, everything still green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(daemon): hook-ingress unix socket + line parsing"
```

---

### Task 8: Config load/default

**Files:**
- Create: `crates/openmicrod/src/config.rs`
- Modify: `crates/openmicrod/src/main.rs`

**Interfaces:**
- Produces: `struct Config { brightness: u8, pinned: Option<(String,String)> }` with
  `fn load() -> Config` (reads `~/.config/openmicro/config.toml`, defaults on missing/invalid),
  `fn default_path() -> PathBuf`.

- [ ] **Step 1: Write the failing test**

In `crates/openmicrod/src/config.rs`:
```rust
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
```

- [ ] **Step 2: Add path + load helpers (same file)**

Append:
```rust
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
```

- [ ] **Step 3: Register module + run tests**

Add `mod config;` to `main.rs`.
Run: `cargo test -p openmicrod config`
Expected: three config tests PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(daemon): TOML config with safe defaults"
```

---

### Task 9: Control socket (snapshot stream for TUI)

**Files:**
- Create: `crates/openmicrod/src/control.rs`
- Modify: `crates/openmicrod/src/main.rs`

**Interfaces:**
- Consumes: `engine::Engine`.
- Produces: `struct Snapshot { sessions: Vec<SnapSession>, owner: Option<String> }`,
  `struct SnapSession { agent, session, state, slot }`, all `Serialize`;
  `fn snapshot(engine: &Engine) -> Snapshot`;
  `async fn serve(path, shared_engine)` emitting one JSON snapshot per second per client.

- [ ] **Step 1: Write the failing snapshot test**

In `crates/openmicrod/src/control.rs`:
```rust
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

    #[test]
    fn snapshot_reflects_engine() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::AwaitingApproval, &mut dev);
        let snap = snapshot(&engine);
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.sessions[0].agent, "claude");
        assert_eq!(snap.sessions[0].state, "awaiting_approval");
        assert_eq!(snap.owner.as_deref(), Some("claude:s1"));
    }
}
```

- [ ] **Step 2: Add the async server (same file)**

Append:
```rust
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

pub async fn serve(path: std::path::PathBuf, engine: Arc<Mutex<Engine>>) -> anyhow::Result<()> {
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    loop {
        let (mut stream, _) = listener.accept().await?;
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
```

- [ ] **Step 3: Register module + run tests**

Add `mod control;` to `main.rs`.
Run: `cargo test -p openmicrod control`
Expected: `snapshot_reflects_engine` PASSES.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(daemon): control socket snapshot stream"
```

---

### Task 10: Wire up the daemon `main`

**Files:**
- Modify: `crates/openmicrod/src/main.rs`

**Interfaces:**
- Consumes: all daemon modules.
- Produces: a running daemon that serves both sockets against a shared `Engine` + `MockDevice`.

- [ ] **Step 1: Write `main`**

`crates/openmicrod/src/main.rs`:
```rust
mod config;
mod control;
mod device;
mod engine;
mod focus;
mod ingress;
mod render;
mod session;

use std::sync::Arc;
use tokio::sync::Mutex;

use device::MockDevice;
use engine::Engine;

fn runtime_dir() -> std::path::PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = config::load();
    let engine = Arc::new(Mutex::new(Engine::new(cfg.brightness)));
    let device: Arc<Mutex<dyn device::DeviceLink + Send>> =
        Arc::new(Mutex::new(MockDevice::new()));

    let rt = runtime_dir();
    let hook_path = rt.join("openmicro.sock");
    let ctl_path = rt.join("openmicro-ctl.sock");

    let ingress = tokio::spawn(ingress::serve(hook_path, engine.clone(), device.clone()));
    let control = tokio::spawn(control::serve(ctl_path, engine.clone()));

    println!("openmicrod running (mock device). Ctrl-C to stop.");
    tokio::select! {
        r = ingress => { r??; }
        r = control => { r??; }
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}
```

- [ ] **Step 2: Add `signal` feature to tokio**

In workspace `Cargo.toml`, extend the tokio features list to include `"signal"`.

- [ ] **Step 3: Build + smoke test**

Run: `cargo run -p openmicrod &` then
`printf '{"agent":"claude","session":"s1","state":"working"}\n' | nc -U "$XDG_RUNTIME_DIR/openmicro.sock"` (or a short Rust/socat equivalent), then
`nc -U "$XDG_RUNTIME_DIR/openmicro-ctl.sock"` and confirm a JSON snapshot with `claude:s1` appears within ~1s. Kill the daemon.
Expected: snapshot shows the session.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(daemon): wire main to serve hook + control sockets"
```

---

### Task 11: `openmicro-hook` CLI

**Files:**
- Modify: `crates/openmicro-hook/src/main.rs`

**Interfaces:**
- Produces: `openmicro-hook --agent <a> --session <s> --state <st>` → writes one JSON line to the hook socket.

- [ ] **Step 1: Write the CLI**

`crates/openmicro-hook/src/main.rs`:
```rust
use std::io::Write;
use std::os::unix::net::UnixStream;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Push an agent state event to openmicrod")]
struct Args {
    #[arg(long)]
    agent: String,
    #[arg(long)]
    session: String,
    #[arg(long)]
    state: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{rt}/openmicro.sock");
    let payload = serde_json::json!({
        "agent": args.agent,
        "session": args.session,
        "state": args.state,
    });
    // Best-effort: if the daemon is down, exit 0 silently so hooks never block agents.
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let mut line = payload.to_string();
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
    }
    Ok(())
}
```

- [ ] **Step 2: Build + manual check against the running daemon**

Run: `cargo run -p openmicrod &` then
`cargo run -p openmicro-hook -- --agent claude --session s1 --state thinking`, then read the control socket and confirm `claude:s1` = `thinking`. Kill daemon.
Expected: snapshot shows the event.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(hook): openmicro-hook CLI writes events to the daemon"
```

---

### Task 12: Claude Code adapter

**Files:**
- Create: `adapters/claude-code/hooks.json`
- Create: `adapters/claude-code/install.md`

**Interfaces:**
- Produces: copy-paste Claude Code hook config that calls `openmicro-hook` on state transitions.

- [ ] **Step 1: Write the hook config**

`adapters/claude-code/hooks.json` (maps Claude Code lifecycle events to states; `$CLAUDE_SESSION_ID` is illustrative — verify the real env var name during install):
```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "openmicro-hook --agent claude --session \"$CLAUDE_SESSION_ID\" --state thinking" } ] }
    ],
    "PreToolUse": [
      { "hooks": [ { "type": "command", "command": "openmicro-hook --agent claude --session \"$CLAUDE_SESSION_ID\" --state working" } ] }
    ],
    "Notification": [
      { "hooks": [ { "type": "command", "command": "openmicro-hook --agent claude --session \"$CLAUDE_SESSION_ID\" --state awaiting_approval" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "openmicro-hook --agent claude --session \"$CLAUDE_SESSION_ID\" --state idle" } ] }
    ]
  }
}
```

- [ ] **Step 2: Write install notes**

`adapters/claude-code/install.md`: explain merging `hooks.json` into `~/.claude/settings.json`, confirm `openmicro-hook` is on `PATH`, and how to verify (`openmicrod` running + a live Claude Code session flips the mapped slot). Note the real session-id env var must be confirmed against the running Claude Code and substituted.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(adapter): Claude Code hook config + install notes"
```

---

### Task 13: TUI live dashboard

**Files:**
- Create: `crates/openmicro/src/client.rs`
- Create: `crates/openmicro/src/ui.rs`
- Modify: `crates/openmicro/src/main.rs`

**Interfaces:**
- Consumes: the control-socket JSON snapshot from Task 9.
- Produces: `struct SnapshotDto { sessions: Vec<SessionDto>, owner: Option<String> }` (Deserialize),
  a background reader task, and a ratatui render loop showing a table of sessions (agent, session, state, slot) with the owner highlighted.

- [ ] **Step 1: Define the DTO + a render unit test**

In `crates/openmicro/src/client.rs`:
```rust
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
}
```

- [ ] **Step 2: Run the parse test**

Run: `cargo test -p openmicro`
Expected: `parses_snapshot_line` PASSES.

- [ ] **Step 3: Write the render function**

In `crates/openmicro/src/ui.rs`:
```rust
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Row, Table};

use crate::client::SnapshotDto;

pub fn render(frame: &mut Frame, snap: &SnapshotDto) {
    let owner = snap.owner.clone().unwrap_or_default();
    let rows: Vec<Row> = snap
        .sessions
        .iter()
        .map(|s| {
            let id = format!("{}:{}", s.agent, s.session);
            let mut row = Row::new(vec![
                s.slot.map(|i| i.to_string()).unwrap_or_default(),
                s.agent.clone(),
                s.state.clone(),
            ]);
            if id == owner {
                row = row.style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
            }
            row
        })
        .collect();

    let table = Table::new(rows, [Constraint::Length(4), Constraint::Length(12), Constraint::Min(10)])
        .header(Row::new(vec!["slot", "agent", "state"]).style(Style::default().add_modifier(Modifier::UNDERLINED)))
        .block(Block::default().borders(Borders::ALL).title(" OpenMicro — agents "));

    frame.render_widget(table, frame.area());
}
```

- [ ] **Step 4: Write the event/read loop in `main`**

`crates/openmicro/src/main.rs`:
```rust
mod client;
mod ui;

use std::io::BufRead;
use std::sync::{Arc, Mutex};

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;

use client::SnapshotDto;

fn main() -> anyhow::Result<()> {
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{rt}/openmicro-ctl.sock");

    let snap = Arc::new(Mutex::new(SnapshotDto::default()));
    {
        let snap = snap.clone();
        std::thread::spawn(move || {
            if let Ok(stream) = std::os::unix::net::UnixStream::connect(&path) {
                let reader = std::io::BufReader::new(stream);
                for line in reader.lines().map_while(Result::ok) {
                    if let Some(parsed) = client::parse_snapshot(&line) {
                        *snap.lock().unwrap() = parsed;
                    }
                }
            }
        });
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    loop {
        {
            let snap = snap.lock().unwrap();
            terminal.draw(|f| ui::render(f, &snap))?;
        }
        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(k) = event::read()? {
                if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
```

- [ ] **Step 5: Manual end-to-end check**

Run `cargo run -p openmicrod &`, then `cargo run -p openmicro-hook -- --agent claude --session s1 --state working`, then `cargo run -p openmicro`. Confirm the TUI shows the row within ~1s and `q` quits. Kill daemon.
Expected: live row appears, owner highlighted.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): live agent dashboard over control socket"
```

---

## Self-Review

- **Spec coverage:** proto (T2) ✓; session/state engine (T3) ✓; focus policy (T4) ✓; state→LED (T5) ✓; DeviceLink + MockDevice (T6) ✓; hook ingress (T7) ✓; config (T8) ✓; control socket (T9) ✓; daemon main (T10) ✓; `openmicro-hook` (T11) ✓; Claude Code adapter (T12) ✓; TUI dashboard (T13) ✓. Deferred to v1.1 per spec: rich effects, input routing, battery/brightness/sleep, real BLE device link — not in this plan by design. Firmware: separate plan (hardware-gated).
- **Placeholders:** none — every code step is complete. The Claude Code session-id env var is explicitly flagged as verify-at-install, not a code gap.
- **Type consistency:** `Engine::on_event`, `DeviceLink::{set_leds,last_frame}`, `pick_owner`, `render_frame`, `snapshot`, `parse_line`, `parse_snapshot` names/signatures are consistent across tasks. `SLOT_COUNT = 6` used throughout.

## Next (not in this plan)
- Firmware plan — after confirming ESP32-S3 + pinout and taking a flash backup.
- v1.1 — swap MockDevice for a `bluer` BLE `DeviceLink`; add effects, input routing, battery.
