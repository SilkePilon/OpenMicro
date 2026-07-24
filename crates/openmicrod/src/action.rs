use openmicro_proto::{AgentState, InputEvent};

use crate::session::SessionKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Approve the awaiting-approval session at this slot.
    Approve(SessionKey),
    /// Interrupt the session at this slot.
    Interrupt(SessionKey),
    /// Change global brightness by this signed delta (clamped later).
    AdjustBrightness(i16),
    /// Move the pinned focus among live sessions (+1 / -1).
    CycleFocus(i8),
}

/// Read-only view the router needs: which SessionKey (if any) is at each slot,
/// and that session's state.
pub struct RouterView<'a> {
    pub slot_session: &'a dyn Fn(usize) -> Option<(SessionKey, AgentState)>,
}

pub const BRIGHTNESS_STEP: i16 = 8;

/// Map one input event to an action. Key press on a slot: Approve if that
/// session is AwaitingApproval, else Interrupt. Encoder -> brightness.
/// Joystick -> cycle focus. Returns None for key releases / empty slots.
pub fn route(event: &InputEvent, view: &RouterView) -> Option<Action> {
    match *event {
        InputEvent::Key { id, pressed: true } => {
            let (key, state) = (view.slot_session)(id as usize)?;
            Some(if state == AgentState::AwaitingApproval {
                Action::Approve(key)
            } else {
                Action::Interrupt(key)
            })
        }
        InputEvent::Key { pressed: false, .. } => None,
        InputEvent::Encoder { delta } => {
            Some(Action::AdjustBrightness(delta as i16 * BRIGHTNESS_STEP))
        }
        InputEvent::Joystick { dir } => Some(Action::CycleFocus(if dir >= 4 { -1 } else { 1 })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(agent: &str, session: &str) -> SessionKey {
        SessionKey { agent: agent.to_string(), session: session.to_string() }
    }

    fn view_with(slots: Vec<Option<(SessionKey, AgentState)>>) -> impl Fn(usize) -> Option<(SessionKey, AgentState)> {
        move |i: usize| slots.get(i).cloned().flatten()
    }

    #[test]
    fn key_press_on_awaiting_approval_slot_approves() {
        let lookup = view_with(vec![Some((key("claude", "s1"), AgentState::AwaitingApproval))]);
        let view = RouterView { slot_session: &lookup };
        let action = route(&InputEvent::Key { id: 0, pressed: true }, &view);
        assert_eq!(action, Some(Action::Approve(key("claude", "s1"))));
    }

    #[test]
    fn key_press_on_working_slot_interrupts() {
        let lookup = view_with(vec![Some((key("claude", "s1"), AgentState::Working))]);
        let view = RouterView { slot_session: &lookup };
        let action = route(&InputEvent::Key { id: 0, pressed: true }, &view);
        assert_eq!(action, Some(Action::Interrupt(key("claude", "s1"))));
    }

    #[test]
    fn key_press_on_empty_slot_is_none() {
        let lookup = view_with(vec![None]);
        let view = RouterView { slot_session: &lookup };
        assert_eq!(route(&InputEvent::Key { id: 0, pressed: true }, &view), None);
    }

    #[test]
    fn key_release_is_none() {
        let lookup = view_with(vec![Some((key("claude", "s1"), AgentState::Working))]);
        let view = RouterView { slot_session: &lookup };
        assert_eq!(route(&InputEvent::Key { id: 0, pressed: false }, &view), None);
    }

    #[test]
    fn encoder_maps_to_brightness_delta() {
        let lookup = view_with(vec![]);
        let view = RouterView { slot_session: &lookup };
        assert_eq!(
            route(&InputEvent::Encoder { delta: 2 }, &view),
            Some(Action::AdjustBrightness(16))
        );
    }

    #[test]
    fn joystick_maps_to_cycle_focus() {
        let lookup = view_with(vec![]);
        let view = RouterView { slot_session: &lookup };
        assert_eq!(
            route(&InputEvent::Joystick { dir: 1 }, &view),
            Some(Action::CycleFocus(1))
        );
        assert_eq!(
            route(&InputEvent::Joystick { dir: 5 }, &view),
            Some(Action::CycleFocus(-1))
        );
    }
}
