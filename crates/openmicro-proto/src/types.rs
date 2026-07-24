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
