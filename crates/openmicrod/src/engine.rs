use std::collections::HashMap;

use openmicro_proto::{AgentState, Command, StateColors, SLOT_COUNT};

use crate::action::Action;
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
    pub fn release(&mut self, key: &SessionKey) {
        self.slots.remove(key);
    }
}

pub struct Engine {
    pub store: SessionStore,
    pub mapping: Mapping,
    pub pinned: Option<SessionKey>,
    pub brightness: u8,
    pub colors: StateColors,
}

impl Engine {
    pub fn new(brightness: u8) -> Self {
        Self {
            store: SessionStore::new(),
            mapping: Mapping::default(),
            pinned: None,
            brightness,
            colors: StateColors::default(),
        }
    }

    /// Fields the caller needs to persist to config after a command.
    pub fn to_config_fields(&self) -> (u8, StateColors) {
        (self.brightness, self.colors)
    }

    /// Apply a TUI command: update engine state and re-render to the device.
    pub async fn apply_command(&mut self, cmd: Command, device: &mut dyn DeviceLink) {
        match cmd {
            Command::SetBrightness(b) => {
                self.brightness = b;
                self.rerender(device).await;
            }
            Command::SetStateColor { state, rgb } => {
                match state {
                    AgentState::Idle => self.colors.idle = rgb,
                    AgentState::Thinking => self.colors.thinking = rgb,
                    AgentState::Working => self.colors.working = rgb,
                    AgentState::AwaitingApproval => self.colors.awaiting_approval = rgb,
                }
                self.rerender(device).await;
            }
        }
    }

    pub async fn on_event(
        &mut self,
        agent: &str,
        session: &str,
        state: AgentState,
        device: &mut dyn DeviceLink,
    ) {
        if state == AgentState::Idle {
            let key = SessionKey { agent: agent.to_string(), session: session.to_string() };
            self.store.remove(&key);
            self.mapping.release(&key);
            self.rerender(device).await;
            return;
        }
        let key = self.store.update(agent, session, state);
        self.mapping.assign(&key);
        self.rerender(device).await;
    }

    /// Read-only lookup of which session (key + state) occupies a given slot.
    /// Used to build a `RouterView` for input routing.
    pub fn slot_lookup(&self) -> impl Fn(usize) -> Option<(SessionKey, AgentState)> + '_ {
        move |slot: usize| {
            self.store.iter().find_map(|s| {
                if self.mapping.slot_for(&s.key) == Some(slot) {
                    Some((s.key.clone(), s.state))
                } else {
                    None
                }
            })
        }
    }

    /// Execute a routed action against the engine + device.
    pub async fn apply_action(&mut self, action: Action, device: &mut dyn DeviceLink) {
        match action {
            Action::AdjustBrightness(delta) => {
                self.brightness = (self.brightness as i16 + delta).clamp(0, 255) as u8;
                self.rerender(device).await;
            }
            Action::CycleFocus(dir) => {
                // Live session keys ordered by their stable slot index.
                let mut keyed: Vec<(usize, SessionKey)> = self
                    .store
                    .iter()
                    .filter_map(|s| self.mapping.slot_for(&s.key).map(|i| (i, s.key.clone())))
                    .collect();
                keyed.sort_by_key(|(i, _)| *i);
                if keyed.is_empty() {
                    return;
                }
                let keys: Vec<SessionKey> = keyed.into_iter().map(|(_, k)| k).collect();

                // Start from the pinned session if set, else the current owner.
                let current = self
                    .pinned
                    .as_ref()
                    .and_then(|p| keys.iter().position(|k| k == p))
                    .or_else(|| {
                        pick_owner(self.store.iter(), self.pinned.as_ref())
                            .and_then(|o| keys.iter().position(|k| *k == o))
                    })
                    .unwrap_or(0);

                let len = keys.len() as i64;
                let next = (current as i64 + dir as i64).rem_euclid(len) as usize;
                self.pinned = Some(keys[next].clone());
                self.rerender(device).await;
            }
            Action::Approve(ref key) | Action::Interrupt(ref key) => {
                // TODO(adapters): execute via per-agent control channel.
                eprintln!("openmicro: action {:?} on {}:{}", action, key.agent, key.session);
            }
        }
    }

    async fn rerender(&self, device: &mut dyn DeviceLink) {
        // v1: each mapped agent has its own key, so every mapped slot is lit
        // with its own state. Focus/owner drives input routing + TUI highlight
        // (see control.rs), not which keys light.
        let mut slots = [None; SLOT_COUNT];
        for session in self.store.iter() {
            if let Some(i) = self.mapping.slot_for(&session.key) {
                slots[i] = Some(session.state);
            }
        }
        let frame = render_frame(&slots, self.brightness, &self.colors);
        device.set_leds(&frame).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MockDevice;
    use openmicro_proto::{Command, Rgb};

    #[tokio::test]
    async fn apply_set_brightness_updates_and_rerenders() {
        let mut engine = Engine::new(100);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        let before = dev.writes;
        engine.apply_command(Command::SetBrightness(50), &mut dev).await;
        assert_eq!(engine.brightness, 50);
        assert!(dev.writes > before);
        assert_eq!(dev.last_frame().slots[0].brightness, 50);
    }

    #[tokio::test]
    async fn apply_set_state_color_changes_render() {
        let mut engine = Engine::new(200);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        engine
            .apply_command(
                Command::SetStateColor {
                    state: AgentState::Working,
                    rgb: Rgb { r: 1, g: 2, b: 3 },
                },
                &mut dev,
            )
            .await;
        assert_eq!(dev.last_frame().slots[0].color, Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(engine.colors.working, Rgb { r: 1, g: 2, b: 3 });
    }

    #[tokio::test]
    async fn hook_event_lights_the_assigned_slot() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        let frame = dev.last_frame();
        // claude:s1 got slot 0.
        assert_eq!(frame.slots[0].color, Rgb { r: 0, g: 200, b: 80 });
        assert_eq!(dev.writes, 1);
    }

    #[tokio::test]
    async fn two_agents_get_distinct_slots() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        engine.on_event("codex", "s2", AgentState::Thinking, &mut dev).await;
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0].color, Rgb { r: 0, g: 200, b: 80 }); // working
        assert_eq!(frame.slots[1].color, Rgb { r: 40, g: 90, b: 255 }); // thinking
    }

    #[tokio::test]
    async fn idle_frees_the_slot() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0].color, Rgb { r: 0, g: 200, b: 80 }); // working

        engine.on_event("claude", "s1", AgentState::Idle, &mut dev).await;
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0], openmicro_proto::LedSlot::OFF);

        // A new session should be able to reuse slot 0 now that it was freed.
        engine.on_event("codex", "s2", AgentState::Working, &mut dev).await;
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0].color, Rgb { r: 0, g: 200, b: 80 });
    }

    #[tokio::test]
    async fn adjust_brightness_changes_engine_and_frame() {
        let mut engine = Engine::new(100);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        engine.on_event("codex", "s2", AgentState::Working, &mut dev).await;

        engine.apply_action(Action::AdjustBrightness(16), &mut dev).await;
        assert_eq!(engine.brightness, 116);
        let frame = dev.last_frame();
        // Both lit slots carry the new brightness.
        assert_eq!(frame.slots[0].brightness, 116);
        assert_eq!(frame.slots[1].brightness, 116);
    }

    #[tokio::test]
    async fn adjust_brightness_clamps_high_and_low() {
        let mut engine = Engine::new(250);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;

        engine.apply_action(Action::AdjustBrightness(16), &mut dev).await;
        assert_eq!(engine.brightness, 255);

        engine.brightness = 4;
        engine.apply_action(Action::AdjustBrightness(-16), &mut dev).await;
        assert_eq!(engine.brightness, 0);
    }

    #[tokio::test]
    async fn cycle_focus_pins_a_live_session() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        // claude:s1 -> slot 0, codex:s2 -> slot 1 (codex is the default owner).
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        std::thread::sleep(std::time::Duration::from_millis(2));
        engine.on_event("codex", "s2", AgentState::Working, &mut dev).await;
        assert!(engine.pinned.is_none());

        engine.apply_action(Action::CycleFocus(1), &mut dev).await;

        let pinned = engine.pinned.clone().expect("focus should be pinned");
        // Owner started at codex:s2 (slot 1); +1 wraps to slot 0 = claude:s1.
        assert_eq!(pinned, SessionKey { agent: "claude".into(), session: "s1".into() });
        let snap = crate::control::snapshot(&engine);
        assert_eq!(snap.owner.as_deref(), Some("claude:s1"));
    }
}
