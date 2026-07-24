use openmicro_proto::{AgentState, Effect, LedFrame, LedSlot, SLOT_COUNT, StateColors};

/// Per-state LED effect. Color now comes from configurable `StateColors`.
pub fn state_effect(state: AgentState) -> Effect {
    match state {
        AgentState::Idle => Effect::Solid,
        AgentState::Thinking => Effect::Breath,
        AgentState::Working => Effect::Solid,
        AgentState::AwaitingApproval => Effect::Pulse,
    }
}

/// Build a frame from per-slot assigned states. `None` = empty slot (off).
pub fn render_frame(
    slots: &[Option<AgentState>; SLOT_COUNT],
    brightness: u8,
    colors: &StateColors,
) -> LedFrame {
    let mut frame = LedFrame::BLANK;
    for (i, maybe_state) in slots.iter().enumerate() {
        if let Some(state) = maybe_state {
            let color = colors.for_state(*state);
            let effect = state_effect(*state);
            frame.slots[i] = LedSlot { color, effect, brightness };
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    use openmicro_proto::Rgb;

    #[test]
    fn assigned_slot_gets_state_color() {
        let mut slots = [None; SLOT_COUNT];
        slots[2] = Some(AgentState::Working);
        let frame = render_frame(&slots, 128, &StateColors::default());
        assert_eq!(frame.slots[2].color, Rgb { r: 0, g: 200, b: 80 });
        assert_eq!(frame.slots[2].brightness, 128);
        assert_eq!(frame.slots[0], LedSlot::OFF);
    }

    #[test]
    fn custom_colors_are_used() {
        let mut slots = [None; SLOT_COUNT];
        slots[0] = Some(AgentState::Working);
        let colors = StateColors { working: Rgb { r: 1, g: 2, b: 3 }, ..Default::default() };
        let frame = render_frame(&slots, 100, &colors);
        assert_eq!(frame.slots[0].color, Rgb { r: 1, g: 2, b: 3 });
    }
}
