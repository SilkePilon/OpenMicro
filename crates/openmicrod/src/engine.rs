use std::collections::HashMap;

use openmicro_proto::{AgentState, SLOT_COUNT};

use crate::device::DeviceLink;
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

    pub async fn on_event(
        &mut self,
        agent: &str,
        session: &str,
        state: AgentState,
        device: &mut dyn DeviceLink,
    ) {
        let key = self.store.update(agent, session, state);
        self.mapping.assign(&key);
        self.rerender(device).await;
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
        let frame = render_frame(&slots, self.brightness);
        device.set_leds(&frame).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MockDevice;
    use openmicro_proto::Rgb;

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
}
