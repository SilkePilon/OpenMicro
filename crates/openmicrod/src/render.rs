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
