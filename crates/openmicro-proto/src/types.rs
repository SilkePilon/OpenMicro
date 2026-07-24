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

/// Per-state RGB colors. Defaults match the historical hardcoded palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateColors {
    pub idle: Rgb,
    pub thinking: Rgb,
    pub working: Rgb,
    pub awaiting_approval: Rgb,
}

impl Default for StateColors {
    fn default() -> Self {
        Self {
            idle: Rgb { r: 0, g: 0, b: 0 },
            thinking: Rgb { r: 40, g: 90, b: 255 },
            working: Rgb { r: 0, g: 200, b: 80 },
            awaiting_approval: Rgb { r: 255, g: 140, b: 0 },
        }
    }
}

impl StateColors {
    pub fn for_state(&self, s: AgentState) -> Rgb {
        match s {
            AgentState::Idle => self.idle,
            AgentState::Thinking => self.thinking,
            AgentState::Working => self.working,
            AgentState::AwaitingApproval => self.awaiting_approval,
        }
    }
}

/// Commands sent from the TUI to the daemon over the control socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    SetBrightness(u8),
    SetStateColor { state: AgentState, rgb: Rgb },
    SetSleepMinutes(u32),
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

    pub fn encode(&self) -> Result<alloc::vec::Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
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

impl InputEvent {
    pub fn encode(&self) -> Result<alloc::vec::Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
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

    #[test]
    fn state_colors_default_for_state() {
        let c = StateColors::default();
        assert_eq!(c.for_state(AgentState::Working), Rgb { r: 0, g: 200, b: 80 });
        assert_eq!(c.for_state(AgentState::Idle), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(c.for_state(AgentState::Thinking), Rgb { r: 40, g: 90, b: 255 });
        assert_eq!(c.for_state(AgentState::AwaitingApproval), Rgb { r: 255, g: 140, b: 0 });
    }

    #[test]
    fn command_json_roundtrips() {
        let cmd = Command::SetStateColor {
            state: AgentState::Working,
            rgb: Rgb { r: 1, g: 2, b: 3 },
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, back);

        let b = Command::SetBrightness(42);
        let back2: Command = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(b, back2);

        let s = Command::SetSleepMinutes(5);
        let back3: Command = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back3);
    }

    #[test]
    fn input_event_roundtrips() {
        for ev in [
            InputEvent::Key { id: 3, pressed: true },
            InputEvent::Key { id: 0, pressed: false },
            InputEvent::Encoder { delta: -5 },
            InputEvent::Joystick { dir: 2 },
        ] {
            let bytes = ev.encode().unwrap();
            let back = InputEvent::decode(&bytes).unwrap();
            assert_eq!(ev, back);
        }
    }
}
