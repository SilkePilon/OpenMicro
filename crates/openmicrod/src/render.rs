//! Turning live sessions into a frame for the device.
//!
//! The visual decisions all live in `openmicro_effects::status` — this module
//! only decides *which* session each part of the board is talking about. That
//! split matters: the firmware runs the same `status` code when the daemon is
//! not there to ask, so there is exactly one definition of what amber means.

use openmicro_effects::status;
use openmicro_proto::{AgentColors, AgentKind, AgentState, LedFrame, LedSlot, SLOT_COUNT};

/// One agent key's worth of state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotView {
    pub kind: AgentKind,
    pub state: AgentState,
}

/// Build a frame from per-slot sessions.
///
/// `focus` is the slot whose state the ring shows — normally the session that
/// owns the deck (see `focus::pick_owner`). `None` for an empty slot.
///
/// Three things happen here, and they are independent on purpose:
/// - each occupied agent key gets its agent's colour and its state's effect;
/// - the ring gets the focused session's state, or a quiet "nothing running"
///   breath when there is none;
/// - the bottom row lights only if the focused session is actually waiting on a
///   decision.
pub fn render_frame(
    slots: &[Option<SlotView>; SLOT_COUNT],
    focus: Option<usize>,
    brightness: u8,
    colors: &AgentColors,
) -> LedFrame {
    let mut frame = LedFrame::BLANK;

    for (i, maybe) in slots.iter().enumerate() {
        if let Some(view) = maybe {
            frame.slots[i] = status::slot_for(colors.for_kind(view.kind), view.state, brightness);
        }
    }

    // The ring follows the focused session. Falling back to "no agents" when
    // focus is empty (rather than leaving the ring dark) is what makes a
    // working-but-idle device distinguishable from a dead one.
    let focused = focus.and_then(|i| slots.get(i).copied().flatten());
    frame.glow = match focused {
        Some(view) => {
            status::agent_glow(colors.for_kind(view.kind), view.state, brightness)
        }
        None => status::no_agents_glow(brightness),
    };

    frame.actions = status::action_keys_for(focused.map(|v| v.state));
    // The transparent-keycap light mirrors the focused session, so the one key
    // that is readable from across the room says who is waiting.
    frame.status = match focused {
        Some(view) => {
            status::status_slot(colors.for_kind(view.kind), Some(view.state), brightness)
        }
        None => LedSlot::OFF,
    };
    frame.brightness = brightness;
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    use openmicro_proto::Glow;

    /// The ring when the daemon has nothing to say yet.
    fn idle_glow(brightness: u8) -> Glow {
        status::no_agents_glow(brightness)
    }

    use openmicro_proto::layout;
    use openmicro_proto::{AgentColors, Effect, Motion, Rgb};

    fn view(kind: AgentKind, state: AgentState) -> Option<SlotView> {
        Some(SlotView { kind, state })
    }

    #[test]
    fn an_assigned_slot_gets_its_agents_color() {
        let mut slots = [None; SLOT_COUNT];
        slots[2] = view(AgentKind::Claude, AgentState::Working);
        let frame = render_frame(&slots, Some(2), 255, &AgentColors::default());
        assert_eq!(frame.slots[2].color, AgentKind::Claude.brand());
        assert_eq!(frame.slots[0], LedSlot::OFF, "empty slots stay dark");
    }

    #[test]
    fn different_agents_in_the_same_state_are_different_colors() {
        // This is the headline behaviour: the board tells you *who*, not just
        // that something is busy.
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Claude, AgentState::Working);
        slots[1] = view(AgentKind::Codex, AgentState::Working);
        slots[2] = view(AgentKind::Grok, AgentState::Working);
        let frame = render_frame(&slots, Some(0), 255, &AgentColors::default());
        assert_eq!(frame.slots[0].color, AgentKind::Claude.brand());
        assert_eq!(frame.slots[1].color, AgentKind::Codex.brand());
        assert_eq!(frame.slots[2].color, AgentKind::Grok.brand());
        assert_ne!(frame.slots[0].color, frame.slots[1].color);
        assert_ne!(frame.slots[1].color, frame.slots[2].color);
    }

    #[test]
    fn the_ring_shows_the_focused_session_not_just_any_session() {
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Claude, AgentState::Working);
        slots[1] = view(AgentKind::Grok, AgentState::Idle);

        let on_claude = render_frame(&slots, Some(0), 255, &AgentColors::default());
        assert_eq!(on_claude.glow.color, AgentKind::Claude.brand());
        assert_eq!(on_claude.glow.motion, Motion::Spin);

        let on_grok = render_frame(&slots, Some(1), 255, &AgentColors::default());
        assert_eq!(on_grok.glow.color, AgentKind::Grok.brand());
        assert_eq!(on_grok.glow.motion, Motion::Breath);
    }

    #[test]
    fn no_sessions_still_shows_a_quiet_ring() {
        // A device that goes fully dark when idle is indistinguishable from one
        // that has crashed.
        let frame = render_frame(&[None; SLOT_COUNT], None, 255, &AgentColors::default());
        assert_ne!(frame.glow.motion, Motion::Off);
        assert!(frame.glow.brightness > 0);
        assert!(!frame.actions.any(), "nothing to decide, so no action keys");
    }

    #[test]
    fn a_focus_pointing_at_an_empty_slot_falls_back_gracefully() {
        let slots = [None; SLOT_COUNT];
        let frame = render_frame(&slots, Some(3), 255, &AgentColors::default());
        assert_eq!(frame.glow, idle_glow(255));
        assert!(!frame.actions.any());
    }

    #[test]
    fn a_focus_beyond_the_slot_array_does_not_panic() {
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Claude, AgentState::Working);
        let frame = render_frame(&slots, Some(999), 255, &AgentColors::default());
        assert_eq!(frame.glow, idle_glow(255));
    }

    #[test]
    fn awaiting_approval_arms_the_bottom_row_and_alerts_the_ring() {
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Claude, AgentState::AwaitingApproval);
        let frame = render_frame(&slots, Some(0), 255, &AgentColors::default());

        assert!(frame.actions.approve && frame.actions.deny);
        assert_eq!(frame.glow.motion, Motion::Alert);
        // The waiting key itself is the only per-key animation, so it is
        // findable among five busy neighbours.
        assert_eq!(frame.slots[0].effect, Effect::Pulse);
    }

    #[test]
    fn a_busy_but_unfocused_waiting_session_does_not_arm_the_row() {
        // The action keys act on the *focused* session. Arming them for a
        // session the ring is not showing would make the press ambiguous.
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Claude, AgentState::Working);
        slots[1] = view(AgentKind::Grok, AgentState::AwaitingApproval);
        let frame = render_frame(&slots, Some(0), 255, &AgentColors::default());
        assert!(!frame.actions.any(), "armed for a session the ring is not showing");
        // ...but slot 1's key still pulses, so you can see it is waiting.
        assert_eq!(frame.slots[1].effect, Effect::Pulse);
    }

    #[test]
    fn nothing_is_armed_for_a_running_session() {
        for state in [AgentState::Idle, AgentState::Thinking, AgentState::Working] {
            let mut slots = [None; SLOT_COUNT];
            slots[0] = view(AgentKind::Claude, state);
            let frame = render_frame(&slots, Some(0), 255, &AgentColors::default());
            assert!(!frame.actions.any(), "{state:?} armed the action row");
        }
    }

    #[test]
    fn brightness_zero_blanks_everything_including_the_ring() {
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Claude, AgentState::AwaitingApproval);
        let frame = render_frame(&slots, Some(0), 0, &AgentColors::default());
        assert_eq!(frame.slots[0].brightness, 0);
        assert_eq!(frame.glow.brightness, 0);
    }

    #[test]
    fn the_whole_board_expands_correctly_for_a_pending_decision() {
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Claude, AgentState::AwaitingApproval);
        let frame = render_frame(&slots, Some(0), 255, &AgentColors::default());
        let keys = status::key_slots(&frame);

        // The agent key carries Claude's colour...
        assert_eq!(keys[0].color, AgentKind::Claude.brand());
        // ...the bottom row reads green, amber, red left to right...
        assert_eq!(keys[layout::APPROVE_KEY as usize].color, openmicro_proto::APPROVE_COLOR);
        assert_eq!(keys[layout::DENY_KEY as usize].color, openmicro_proto::DENY_COLOR);
        // ...the transparent keycap flashes in Claude's colour...
        assert_eq!(keys[layout::STATUS_KEY as usize].color, AgentKind::Claude.brand());
        assert_eq!(keys[layout::STATUS_KEY as usize].effect, Effect::Pulse);
        // ...nothing is running, so stop stays dark...
        assert_eq!(keys[layout::INTERRUPT_KEY as usize], LedSlot::OFF);
        // ...and the unused keys stay dark.
        for id in layout::RESERVED_KEYS {
            assert_eq!(keys[id as usize], LedSlot::OFF, "reserved key {id} lit up");
        }
    }

    #[test]
    fn a_working_session_lights_only_the_stop_key() {
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Grok, AgentState::Working);
        let frame = render_frame(&slots, Some(0), 255, &AgentColors::default());
        let keys = status::key_slots(&frame);

        assert_eq!(keys[layout::INTERRUPT_KEY as usize].color, openmicro_proto::INTERRUPT_COLOR);
        for id in layout::ACTION_KEYS {
            assert_eq!(keys[id as usize], LedSlot::OFF, "decision key {id} lit with no decision");
        }
    }

    #[test]
    fn an_idle_board_lights_no_control_keys_at_all() {
        let frame = render_frame(&[None; SLOT_COUNT], None, 255, &AgentColors::default());
        let keys = status::key_slots(&frame);
        for id in layout::INTERRUPT_KEY..layout::KEY_COUNT as u8 {
            assert_eq!(keys[id as usize], LedSlot::OFF, "key {id} lit while idle");
        }
    }

    #[test]
    fn an_unknown_agent_gets_the_neutral_color_not_claudes() {
        let mut slots = [None; SLOT_COUNT];
        slots[0] = view(AgentKind::Other, AgentState::Working);
        let frame = render_frame(&slots, Some(0), 255, &AgentColors::default());
        assert_eq!(frame.slots[0].color, AgentKind::Other.brand());
        assert_ne!(frame.slots[0].color, AgentKind::Claude.brand());
        assert_ne!(frame.slots[0].color, Rgb { r: 0, g: 0, b: 0 });
    }
}
