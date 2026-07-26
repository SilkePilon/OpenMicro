use openmicro_proto::layout::{self, KeyRole};
use openmicro_proto::{AgentState, InputEvent};

use crate::session::SessionKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Approve(SessionKey),
    Deny(SessionKey),
    Interrupt(SessionKey),
    FocusSlot(usize),
    AdjustBrightness(i16),
    CycleFocus(i8),
}

pub struct RouterView<'a> {
    pub slot_session: &'a dyn Fn(usize) -> Option<(SessionKey, AgentState)>,
    pub focus: Option<(SessionKey, AgentState)>,
}

pub const BRIGHTNESS_STEP: i16 = 8;

pub fn route(event: &InputEvent, view: &RouterView) -> Option<Action> {
    match *event {
        InputEvent::Key { id, pressed: true } => route_key(id, view),
        InputEvent::Key { pressed: false, .. } => None,
        InputEvent::Encoder { delta } => {
            Some(Action::AdjustBrightness(delta as i16 * BRIGHTNESS_STEP))
        }
        InputEvent::Joystick { dir } => Some(Action::CycleFocus(if dir >= 4 { -1 } else { 1 })),
    }
}

fn route_key(id: u8, view: &RouterView) -> Option<Action> {
    match layout::role_of(id)? {
        KeyRole::Agent(slot) => {
            (view.slot_session)(slot as usize)?;
            Some(Action::FocusSlot(slot as usize))
        }
        KeyRole::Reserved => None,
        KeyRole::Interrupt => {
            let (key, state) = view.focus.clone()?;
            matches!(state, AgentState::Thinking | AgentState::Working)
                .then(|| Action::Interrupt(key))
        }
        KeyRole::Approve => decide(view, Action::Approve),
        KeyRole::Deny => decide(view, Action::Deny),
        KeyRole::Status => None,
    }
}

fn decide(view: &RouterView, make: fn(SessionKey) -> Action) -> Option<Action> {
    let (key, state) = view.focus.clone()?;
    (state == AgentState::AwaitingApproval).then(|| make(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(agent: &str, session: &str) -> SessionKey {
        SessionKey { agent: agent.to_string(), session: session.to_string() }
    }

    fn lookup(
        slots: Vec<Option<(SessionKey, AgentState)>>,
    ) -> impl Fn(usize) -> Option<(SessionKey, AgentState)> {
        move |i: usize| slots.get(i).cloned().flatten()
    }

    fn focused_on(state: AgentState) -> (impl Fn(usize) -> Option<(SessionKey, AgentState)>, SessionKey) {
        let k = key("claude", "s1");
        (lookup(vec![Some((k.clone(), state))]), k)
    }

    #[test]
    fn the_bottom_row_decides_a_waiting_request() {
        let (look, k) = focused_on(AgentState::AwaitingApproval);
        let view = RouterView {
            slot_session: &look,
            focus: Some((k.clone(), AgentState::AwaitingApproval)),
        };
        let press = |id| route(&InputEvent::Key { id, pressed: true }, &view);

        assert_eq!(press(layout::APPROVE_KEY), Some(Action::Approve(k.clone())));
        assert_eq!(press(layout::DENY_KEY), Some(Action::Deny(k)));
        assert_eq!(press(layout::STATUS_KEY), None);
    }

    #[test]
    fn the_bottom_row_does_nothing_when_no_decision_is_pending() {
        for state in [AgentState::Idle, AgentState::Thinking, AgentState::Working] {
            let (look, k) = focused_on(state);
            let view = RouterView { slot_session: &look, focus: Some((k, state)) };
            for id in layout::ACTION_KEYS {
                assert_eq!(
                    route(&InputEvent::Key { id, pressed: true }, &view),
                    None,
                    "key {id} acted while {state:?}"
                );
            }
        }
    }

    #[test]
    fn the_bottom_row_does_nothing_with_nothing_selected() {
        let look = lookup(vec![]);
        let view = RouterView { slot_session: &look, focus: None };
        for id in layout::ACTION_KEYS {
            assert_eq!(route(&InputEvent::Key { id, pressed: true }, &view), None);
        }
    }

    #[test]
    fn an_agent_key_selects_its_slot() {
        let k = key("claude", "s1");
        let look = lookup(vec![None, Some((k.clone(), AgentState::Working))]);
        let view = RouterView { slot_session: &look, focus: None };
        assert_eq!(
            route(&InputEvent::Key { id: layout::AGENT_KEYS[1], pressed: true }, &view),
            Some(Action::FocusSlot(1))
        );
    }

    #[test]
    fn an_empty_agent_key_selects_nothing() {
        let look = lookup(vec![None, None]);
        let view = RouterView { slot_session: &look, focus: None };
        for slot in 0..layout::AGENT_KEYS.len() {
            assert_eq!(
                route(&InputEvent::Key { id: layout::AGENT_KEYS[slot], pressed: true }, &view),
                None,
                "empty slot {slot} was selectable"
            );
        }
    }

    #[test]
    fn reserved_keys_do_nothing() {
        let (look, k) = focused_on(AgentState::AwaitingApproval);
        let view = RouterView {
            slot_session: &look,
            focus: Some((k, AgentState::AwaitingApproval)),
        };
        for id in layout::RESERVED_KEYS {
            assert_eq!(
                route(&InputEvent::Key { id, pressed: true }, &view),
                None,
                "reserved key {id} did something"
            );
        }
    }

    #[test]
    fn a_key_off_the_board_does_nothing() {
        let (look, k) = focused_on(AgentState::AwaitingApproval);
        let view = RouterView {
            slot_session: &look,
            focus: Some((k, AgentState::AwaitingApproval)),
        };
        for id in [13u8, 40, 0xFE, 0xFF] {
            assert_eq!(route(&InputEvent::Key { id, pressed: true }, &view), None, "id {id}");
        }
    }

    #[test]
    fn a_release_never_acts() {
        let (look, k) = focused_on(AgentState::AwaitingApproval);
        let view = RouterView {
            slot_session: &look,
            focus: Some((k, AgentState::AwaitingApproval)),
        };
        for id in layout::ACTION_KEYS {
            assert_eq!(route(&InputEvent::Key { id, pressed: false }, &view), None);
        }
    }

    #[test]
    fn approve_and_deny_are_never_the_same_action() {
        let (look, k) = focused_on(AgentState::AwaitingApproval);
        let view = RouterView {
            slot_session: &look,
            focus: Some((k, AgentState::AwaitingApproval)),
        };
        let approve = route(&InputEvent::Key { id: layout::APPROVE_KEY, pressed: true }, &view);
        let deny = route(&InputEvent::Key { id: layout::DENY_KEY, pressed: true }, &view);
        assert_ne!(approve, deny);
        assert!(matches!(approve, Some(Action::Approve(_))));
        assert!(matches!(deny, Some(Action::Deny(_))));
    }

    #[test]
    fn encoder_maps_to_brightness_delta() {
        let look = lookup(vec![]);
        let view = RouterView { slot_session: &look, focus: None };
        assert_eq!(
            route(&InputEvent::Encoder { delta: 2 }, &view),
            Some(Action::AdjustBrightness(16))
        );
        assert_eq!(
            route(&InputEvent::Encoder { delta: -1 }, &view),
            Some(Action::AdjustBrightness(-8))
        );
    }

    #[test]
    fn joystick_maps_to_cycle_focus() {
        let look = lookup(vec![]);
        let view = RouterView { slot_session: &look, focus: None };
        assert_eq!(route(&InputEvent::Joystick { dir: 1 }, &view), Some(Action::CycleFocus(1)));
        assert_eq!(route(&InputEvent::Joystick { dir: 5 }, &view), Some(Action::CycleFocus(-1)));
    }
}
