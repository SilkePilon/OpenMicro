use std::collections::HashMap;

use openmicro_proto::{AgentColors, AgentState, Command, LedFrame, SLOT_COUNT};

use crate::action::Action;
use crate::device::DeviceLink;
use crate::focus::pick_owner;
use crate::render::{render_frame, SlotView};
use crate::session::{SessionKey, SessionStore};

/// Upper bound on `sleep_minutes` (24h). Mirrored in the TUI's `adjust()`.
pub(crate) const MAX_SLEEP_MINUTES: u32 = 1440;

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
    pub colors: AgentColors,
    /// When true, `rerender` blanks the LEDs regardless of live sessions.
    pub asleep: bool,
    /// Idle minutes before the daemon puts the LEDs to sleep (0 disables).
    pub sleep_minutes: u32,
}

impl Engine {
    pub fn new(brightness: u8) -> Self {
        Self {
            store: SessionStore::new(),
            mapping: Mapping::default(),
            pinned: None,
            brightness,
            colors: AgentColors::default(),
            asleep: false,
            sleep_minutes: 3,
        }
    }

    /// Fields the caller needs to persist to config after a command.
    pub fn to_config_fields(&self) -> (u8, AgentColors, u32) {
        (self.brightness, self.colors, self.sleep_minutes)
    }

    /// Put the LEDs to sleep: blank the device and mark the engine asleep.
    /// Wired into the idle-sleep timer in the daemon.
    pub async fn sleep(&mut self, device: &mut dyn DeviceLink) {
        self.asleep = true;
        self.rerender(device).await;
    }

    /// Wake the LEDs if asleep, re-rendering live state. Returns whether it woke.
    /// Activity paths wake inline; this is the explicit-wake entry point.
    #[allow(dead_code)]
    pub async fn wake(&mut self, device: &mut dyn DeviceLink) -> bool {
        if !self.asleep {
            return false;
        }
        self.asleep = false;
        self.rerender(device).await;
        true
    }

    /// Apply a TUI command: update engine state and re-render to the device.
    pub async fn apply_command(&mut self, cmd: Command, device: &mut dyn DeviceLink) {
        // Any command counts as activity: wake before showing live state.
        self.asleep = false;
        match cmd {
            Command::SetBrightness(b) => {
                self.brightness = b;
                self.rerender(device).await;
            }
            Command::SetAgentColor { agent, rgb } => {
                self.colors.set(agent, rgb);
                self.rerender(device).await;
            }
            Command::SetSleepMinutes(m) => {
                // Clamp to 24h: a stuck key (or a bogus Command) can't drive
                // this arbitrarily high. Mirrored in the TUI's own adjust().
                self.sleep_minutes = m.min(MAX_SLEEP_MINUTES);
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
        // Any hook event counts as activity: wake so the rerender shows it.
        self.asleep = false;
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

    /// Which slot the ring and the bottom row are talking about.
    ///
    /// One definition, used both to render the frame and to route a press, so
    /// the key that lights up and the session that gets approved cannot drift
    /// apart.
    pub fn focus_slot(&self) -> Option<usize> {
        let owner = pick_owner(self.store.iter(), self.pinned.as_ref())?;
        self.mapping.slot_for(&owner)
    }

    /// The selected session and its state.
    pub fn focused(&self) -> Option<(SessionKey, AgentState)> {
        let owner = pick_owner(self.store.iter(), self.pinned.as_ref())?;
        let state = self.store.iter().find(|s| s.key == owner)?.state;
        Some((owner, state))
    }

    /// The frame the device should be showing right now.
    pub fn frame(&self) -> LedFrame {
        if self.asleep {
            return LedFrame::BLANK;
        }
        let mut slots = [None; SLOT_COUNT];
        for session in self.store.iter() {
            if let Some(i) = self.mapping.slot_for(&session.key) {
                slots[i] = Some(SlotView { kind: session.kind(), state: session.state });
            }
        }
        render_frame(&slots, self.focus_slot(), self.brightness, &self.colors)
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
        // Physical input counts as activity: wake before applying.
        self.asleep = false;
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
            Action::FocusSlot(slot) => {
                // Pin explicitly, so a deliberate press is not immediately
                // overridden by the next session to report activity.
                // Resolved into an owned key before assigning, so the borrow
                // `slot_lookup` holds on `self` has ended.
                let key = (self.slot_lookup())(slot).map(|(k, _)| k);
                if let Some(key) = key {
                    self.pinned = Some(key);
                    self.rerender(device).await;
                }
            }
            Action::Approve(ref key)
            | Action::Deny(ref key)
            | Action::Interrupt(ref key) => {
                // TODO(adapters): execute via per-agent control channel. The
                // decision is routed and logged correctly; what is missing is a
                // way to speak back to a running CLI, which needs one control
                // channel per agent and is tracked separately.
                eprintln!("openmicro: action {:?} on {}:{}", action, key.agent, key.session);
            }
        }
    }

    /// Resend the current frame unchanged.
    ///
    /// The device treats a gap in frames as "the daemon is gone" and switches to
    /// its own animation, so a daemon with nothing to report still has to say
    /// so. See `openmicro_proto::HEARTBEAT_MS`.
    pub async fn heartbeat(&self, device: &mut dyn DeviceLink) {
        self.rerender(device).await;
    }

    async fn rerender(&self, device: &mut dyn DeviceLink) {
        device.set_leds(&self.frame()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MockDevice;
    use openmicro_proto::{AgentKind, Command, Rgb};

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
    async fn apply_set_agent_color_changes_render() {
        let mut engine = Engine::new(200);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        engine
            .apply_command(
                Command::SetAgentColor {
                    agent: openmicro_proto::AgentKind::Claude,
                    rgb: Rgb { r: 1, g: 2, b: 3 },
                },
                &mut dev,
            )
            .await;
        assert_eq!(dev.last_frame().slots[0].color, Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(engine.colors.claude, Rgb { r: 1, g: 2, b: 3 });
    }

    #[tokio::test]
    async fn retuning_one_agent_leaves_another_sessions_key_alone() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        engine.on_event("grok", "s2", AgentState::Working, &mut dev).await;
        engine
            .apply_command(
                Command::SetAgentColor {
                    agent: openmicro_proto::AgentKind::Claude,
                    rgb: Rgb { r: 1, g: 2, b: 3 },
                },
                &mut dev,
            )
            .await;
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0].color, Rgb { r: 1, g: 2, b: 3 }, "claude retuned");
        assert_eq!(
            frame.slots[1].color,
            openmicro_proto::AgentKind::Grok.brand(),
            "grok untouched"
        );
    }

    #[tokio::test]
    async fn hook_event_lights_the_assigned_slot() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        let frame = dev.last_frame();
        // claude:s1 got slot 0, and the key carries Claude's colour rather
        // than a per-state one.
        assert_eq!(frame.slots[0].color, AgentKind::Claude.brand());
        assert_eq!(dev.writes, 1);
    }

    #[tokio::test]
    async fn two_agents_get_distinct_slots() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        engine.on_event("codex", "s2", AgentState::Thinking, &mut dev).await;
        let frame = dev.last_frame();
        // Both are lit in their own agent's colour, so two sessions in
        // different states are still told apart by *who* they are.
        assert_eq!(frame.slots[0].color, AgentKind::Claude.brand());
        assert_eq!(frame.slots[1].color, AgentKind::Codex.brand());
    }

    #[tokio::test]
    async fn idle_frees_the_slot() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0].color, AgentKind::Claude.brand());

        engine.on_event("claude", "s1", AgentState::Idle, &mut dev).await;
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0], openmicro_proto::LedSlot::OFF);

        // A new session should be able to reuse slot 0 now that it was freed.
        engine.on_event("codex", "s2", AgentState::Working, &mut dev).await;
        let frame = dev.last_frame();
        assert_eq!(frame.slots[0].color, AgentKind::Codex.brand());
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
    async fn sleep_blanks_and_activity_wakes() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        assert_ne!(dev.last_frame().slots[0], openmicro_proto::LedSlot::OFF);

        engine.sleep(&mut dev).await;
        assert!(engine.asleep);
        let frame = dev.last_frame();
        for slot in frame.slots {
            assert_eq!(slot, openmicro_proto::LedSlot::OFF);
        }

        // A hook event is activity: it wakes and re-lights the slot.
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        assert!(!engine.asleep);
        assert_eq!(dev.last_frame().slots[0].color, AgentKind::Claude.brand());
    }

    #[tokio::test]
    async fn wake_returns_whether_it_woke() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.on_event("claude", "s1", AgentState::Working, &mut dev).await;
        assert!(!engine.wake(&mut dev).await); // already awake
        engine.sleep(&mut dev).await;
        assert!(engine.wake(&mut dev).await); // was asleep
        assert!(!engine.asleep);
        assert_eq!(dev.last_frame().slots[0].color, AgentKind::Claude.brand());
    }

    #[tokio::test]
    async fn set_sleep_minutes_updates_field() {
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.apply_command(Command::SetSleepMinutes(12), &mut dev).await;
        assert_eq!(engine.sleep_minutes, 12);
        assert_eq!(engine.to_config_fields().2, 12);
    }

    #[tokio::test]
    async fn set_sleep_minutes_clamps_to_24h() {
        // Fix 5: a stuck key (or a bogus Command) can't drive sleep_minutes
        // arbitrarily high.
        let mut engine = Engine::new(255);
        let mut dev = MockDevice::new();
        engine.apply_command(Command::SetSleepMinutes(u32::MAX), &mut dev).await;
        assert_eq!(engine.sleep_minutes, 1440);
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
        let snap = crate::control::snapshot(&engine, None);
        assert_eq!(snap.owner.as_deref(), Some("claude:s1"));
    }
}
