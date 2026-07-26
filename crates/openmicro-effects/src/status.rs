use openmicro_proto::{
    ActionKeys, AgentState, Effect, Glow, LedFrame, LedSlot, Motion, Rgb, NOMINAL_SPEED,
};

use openmicro_proto::layout::{self, ActionRole, KeyRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Offline,
    NoDaemon,
    Live,
}

pub fn link_state(host_attached: bool, since_last_frame_ms: u32) -> Link {
    if since_last_frame_ms < openmicro_proto::DAEMON_TIMEOUT_MS {
        Link::Live
    } else if host_attached {
        Link::NoDaemon
    } else {
        Link::Offline
    }
}

const IDLE_PCT: u32 = 62;
const THINKING_PCT: u32 = 85;
const WORKING_PCT: u32 = 100;
const AWAITING_PCT: u32 = 100;

const IDLE_SPEED: u8 = 96;
const THINKING_SPEED: u8 = NOMINAL_SPEED;
const WORKING_SPEED: u8 = 190;

const NO_AGENTS_PCT: u32 = 40;
const NO_AGENTS_COLOR: Rgb = Rgb { r: 200, g: 220, b: 255 };

const NO_DAEMON_COLOR: Rgb = Rgb { r: 255, g: 150, b: 0 };
const NO_DAEMON_PCT: u32 = 100;

const OFFLINE_COLOR: Rgb = Rgb { r: 0, g: 120, b: 200 };
const OFFLINE_PCT: u32 = 85;

fn pct(brightness: u8, percent: u32) -> u8 {
    ((brightness as u32 * percent) / 100) as u8
}

pub fn local_glow(link: Link, brightness: u8) -> Glow {
    match link {
        Link::Offline => Glow {
            color: OFFLINE_COLOR,
            motion: Motion::Aurora,
            brightness: pct(brightness, OFFLINE_PCT),
            speed: NOMINAL_SPEED,
        },
        Link::NoDaemon => Glow {
            color: NO_DAEMON_COLOR,
            motion: Motion::Searching,
            brightness: pct(brightness, NO_DAEMON_PCT),
            speed: NOMINAL_SPEED,
        },
        Link::Live => Glow::OFF,
    }
}

pub fn agent_glow(color: Rgb, state: AgentState, brightness: u8) -> Glow {
    let (motion, percent, speed) = match state {
        AgentState::Idle => (Motion::Breath, IDLE_PCT, IDLE_SPEED),
        AgentState::Thinking => (Motion::Spin, THINKING_PCT, THINKING_SPEED),
        AgentState::Working => (Motion::Spin, WORKING_PCT, WORKING_SPEED),
        AgentState::AwaitingApproval => (Motion::Alert, AWAITING_PCT, NOMINAL_SPEED),
    };
    Glow { color, motion, brightness: pct(brightness, percent), speed }
}

pub fn no_agents_glow(brightness: u8) -> Glow {
    Glow {
        color: NO_AGENTS_COLOR,
        motion: Motion::Breath,
        brightness: pct(brightness, NO_AGENTS_PCT),
        speed: IDLE_SPEED,
    }
}

pub fn slot_effect(state: AgentState) -> Effect {
    match state {
        AgentState::Idle => Effect::Breath,
        AgentState::Thinking => Effect::Breath,
        AgentState::Working => Effect::Solid,
        AgentState::AwaitingApproval => Effect::Pulse,
    }
}

pub fn slot_for(color: Rgb, state: AgentState, brightness: u8) -> LedSlot {
    let percent = match state {
        AgentState::Idle => IDLE_PCT,
        AgentState::Thinking => THINKING_PCT,
        AgentState::Working => WORKING_PCT,
        AgentState::AwaitingApproval => AWAITING_PCT,
    };
    LedSlot { color, effect: slot_effect(state), brightness: pct(brightness, percent) }
}

pub fn action_slot(role: ActionRole, armed: bool, brightness: u8) -> LedSlot {
    if !armed {
        return LedSlot::OFF;
    }
    LedSlot { color: role.color(), effect: Effect::Solid, brightness }
}

const INTERRUPT_PCT: u32 = 72;

pub fn interrupt_slot(armed: bool, brightness: u8) -> LedSlot {
    if !armed {
        return LedSlot::OFF;
    }
    LedSlot {
        color: openmicro_proto::INTERRUPT_COLOR,
        effect: Effect::Solid,
        brightness: pct(brightness, INTERRUPT_PCT),
    }
}

pub fn key_slots(frame: &LedFrame) -> [LedSlot; layout::KEY_COUNT] {
    let b = frame.brightness;
    let mut keys = [LedSlot::OFF; layout::KEY_COUNT];
    for (id, out) in keys.iter_mut().enumerate() {
        let Some(role) = layout::role_of(id as u8) else { continue };
        *out = match role {
            KeyRole::Agent(slot) => frame.slots[slot as usize],
            KeyRole::Interrupt => interrupt_slot(frame.actions.interrupt, b),
            KeyRole::Reserved => LedSlot::OFF,
            KeyRole::Approve => action_slot(ActionRole::Approve, frame.actions.approve, b),
            KeyRole::Deny => action_slot(ActionRole::Deny, frame.actions.deny, b),
            KeyRole::Status => frame.status,
        };
    }
    keys
}

pub fn status_slot(color: Rgb, state: Option<AgentState>, brightness: u8) -> LedSlot {
    let Some(state) = state else { return LedSlot::OFF };
    LedSlot { color, effect: slot_effect(state), brightness: pct(brightness, STATUS_PCT) }
}

const STATUS_PCT: u32 = 100;

pub fn action_keys_for(state: Option<AgentState>) -> ActionKeys {
    match state {
        Some(AgentState::AwaitingApproval) => {
            ActionKeys { approve: true, deny: true, interrupt: false }
        }
        Some(AgentState::Thinking) | Some(AgentState::Working) => {
            ActionKeys { interrupt: true, ..ActionKeys::NONE }
        }
        _ => ActionKeys::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmicro_proto::AgentKind;

    fn ag(kind: AgentKind, state: AgentState, brightness: u8) -> Glow {
        agent_glow(kind.brand(), state, brightness)
    }

    fn sl(kind: AgentKind, state: AgentState, brightness: u8) -> LedSlot {
        slot_for(kind.brand(), state, brightness)
    }

    const ALL_STATES: [AgentState; 4] = [
        AgentState::Idle,
        AgentState::Thinking,
        AgentState::Working,
        AgentState::AwaitingApproval,
    ];

    #[test]
    fn a_recent_frame_means_the_daemon_is_driving() {
        assert_eq!(link_state(true, 0), Link::Live);
        assert_eq!(link_state(true, openmicro_proto::HEARTBEAT_MS as u32 * 3), Link::Live);
    }

    #[test]
    fn frames_stopping_while_a_host_is_attached_means_the_daemon_died() {
        assert_eq!(link_state(true, openmicro_proto::DAEMON_TIMEOUT_MS), Link::NoDaemon);
        assert_eq!(link_state(true, u32::MAX), Link::NoDaemon);
    }

    #[test]
    fn no_host_and_no_frames_means_offline() {
        assert_eq!(link_state(false, openmicro_proto::DAEMON_TIMEOUT_MS), Link::Offline);
        assert_eq!(link_state(false, u32::MAX), Link::Offline);
    }

    #[test]
    fn arriving_frames_outrank_a_missing_usb_detect() {
        assert_eq!(link_state(false, 0), Link::Live);
    }

    #[test]
    fn offline_and_no_daemon_never_look_the_same() {
        let off = local_glow(Link::Offline, 255);
        let nod = local_glow(Link::NoDaemon, 255);
        assert_ne!(off.motion, nod.motion);
        assert_ne!(off.color, nod.color);
    }

    #[test]
    fn neither_offline_state_is_dark() {
        for link in [Link::Offline, Link::NoDaemon] {
            let g = local_glow(link, 255);
            assert!(g.brightness > 0, "{link:?} rendered dark");
            assert_ne!(g.motion, Motion::Off, "{link:?} rendered as Off");
        }
    }

    #[test]
    fn live_has_no_local_answer() {
        assert_eq!(local_glow(Link::Live, 255), Glow::OFF);
    }

    #[test]
    fn every_agent_state_gets_a_distinct_looking_ring() {
        for (i, a) in ALL_STATES.iter().enumerate() {
            for b in ALL_STATES.iter().skip(i + 1) {
                let ga = ag(AgentKind::Claude, *a, 255);
                let gb = ag(AgentKind::Claude, *b, 255);
                assert_ne!(ga, gb, "{a:?} and {b:?} look identical on the ring");
            }
        }
    }

    #[test]
    fn the_ring_carries_the_agents_own_color() {
        for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Grok] {
            for state in ALL_STATES {
                assert_eq!(
                    ag(kind, state, 255).color,
                    kind.brand(),
                    "{kind:?}/{state:?} lost its brand colour"
                );
            }
        }
    }

    #[test]
    fn running_states_spin_and_waiting_alerts() {
        assert_eq!(ag(AgentKind::Claude, AgentState::Thinking, 255).motion, Motion::Spin);
        assert_eq!(ag(AgentKind::Claude, AgentState::Working, 255).motion, Motion::Spin);
        assert_eq!(ag(AgentKind::Claude, AgentState::Idle, 255).motion, Motion::Breath);
        assert_eq!(
            ag(AgentKind::Claude, AgentState::AwaitingApproval, 255).motion,
            Motion::Alert
        );
    }

    #[test]
    fn working_spins_faster_than_thinking() {
        let thinking = ag(AgentKind::Claude, AgentState::Thinking, 255);
        let working = ag(AgentKind::Claude, AgentState::Working, 255);
        assert_eq!(thinking.motion, working.motion, "same shape");
        assert!(working.speed > thinking.speed, "different urgency");
    }

    #[test]
    fn idle_is_dimmer_than_working() {
        let idle = ag(AgentKind::Claude, AgentState::Idle, 255);
        let working = ag(AgentKind::Claude, AgentState::Working, 255);
        assert!(idle.brightness < working.brightness);
    }

    #[test]
    fn brightness_scales_everything_and_zero_means_off() {
        for state in ALL_STATES {
            let g = ag(AgentKind::Claude, state, 0);
            assert_eq!(g.brightness, 0, "{state:?} ignored a zero brightness");
            let s = sl(AgentKind::Claude, state, 0);
            assert_eq!(s.brightness, 0);
        }
        for link in [Link::Offline, Link::NoDaemon] {
            assert_eq!(local_glow(link, 0).brightness, 0);
        }
        assert_eq!(no_agents_glow(0).brightness, 0);
    }

    #[test]
    fn no_agents_is_quieter_than_any_running_agent() {
        let quiet = no_agents_glow(255);
        for state in ALL_STATES {
            assert!(
                quiet.brightness < ag(AgentKind::Claude, state, 255).brightness,
                "idle-ring outshines {state:?}"
            );
        }
    }

    #[test]
    fn slots_are_colored_by_agent_not_by_state() {
        for state in ALL_STATES {
            assert_eq!(sl(AgentKind::Claude, state, 255).color, AgentKind::Claude.brand());
            assert_eq!(sl(AgentKind::Grok, state, 255).color, AgentKind::Grok.brand());
        }
        let a = sl(AgentKind::Claude, AgentState::Working, 255);
        let b = sl(AgentKind::Codex, AgentState::Working, 255);
        assert_ne!(a.color, b.color);
        assert_eq!(a.effect, b.effect);
    }

    #[test]
    fn only_the_waiting_state_animates_a_key() {
        assert_eq!(slot_effect(AgentState::AwaitingApproval), Effect::Pulse);
        for state in [AgentState::Working, AgentState::Idle, AgentState::Thinking] {
            assert_ne!(slot_effect(state), Effect::Pulse, "{state:?} must not pulse");
        }
    }

    #[test]
    fn action_keys_appear_only_when_a_decision_is_pending() {
        let armed = action_keys_for(Some(AgentState::AwaitingApproval));
        assert!(armed.approve && armed.deny);

        for state in [AgentState::Idle, AgentState::Thinking, AgentState::Working] {
            assert!(
                !action_keys_for(Some(state)).any(),
                "{state:?} lit the action row with nothing to decide"
            );
        }
        assert!(!action_keys_for(None).any(), "no session must mean no action keys");
    }

    #[test]
    fn the_stop_key_tracks_work_in_progress_not_pending_questions() {
        for state in [AgentState::Thinking, AgentState::Working] {
            assert!(action_keys_for(Some(state)).interrupt, "{state:?} should be stoppable");
        }
        assert!(!action_keys_for(Some(AgentState::AwaitingApproval)).interrupt);
        assert!(!action_keys_for(Some(AgentState::Idle)).interrupt);
        assert!(!action_keys_for(None).interrupt);
    }

    #[test]
    fn stop_and_deny_never_light_at_the_same_time() {
        for state in [
            AgentState::Idle,
            AgentState::Thinking,
            AgentState::Working,
            AgentState::AwaitingApproval,
        ] {
            let k = action_keys_for(Some(state));
            assert!(!(k.deny && k.interrupt), "{state:?} lit both red keys");
        }
    }

    #[test]
    fn the_stop_key_is_dimmer_than_the_decision_row() {
        let stop = interrupt_slot(true, 255);
        let deny = action_slot(ActionRole::Deny, true, 255);
        assert!(stop.brightness < deny.brightness, "stop should not shout over a prompt");
        assert_eq!(interrupt_slot(false, 255), LedSlot::OFF);
    }

    #[test]
    fn any_means_a_pending_decision_not_merely_a_lit_key() {
        let stopping = action_keys_for(Some(AgentState::Working));
        assert!(stopping.interrupt);
        assert!(!stopping.any(), "a stoppable session is not a pending decision");
    }

    #[test]
    fn an_unarmed_action_key_is_fully_off() {
        for role in [ActionRole::Approve, ActionRole::Deny] {
            assert_eq!(action_slot(role, false, 255), LedSlot::OFF, "{role:?}");
        }
    }

    #[test]
    fn armed_action_keys_are_steady_and_correctly_colored() {
        let approve = action_slot(ActionRole::Approve, true, 255);
        let deny = action_slot(ActionRole::Deny, true, 255);
        assert_eq!(approve.color, openmicro_proto::APPROVE_COLOR);
        assert_eq!(deny.color, openmicro_proto::DENY_COLOR);
        assert_eq!(approve.effect, Effect::Solid);
        assert_eq!(deny.effect, Effect::Solid);
        assert!(approve.brightness > 0 && deny.brightness > 0);
    }

    #[test]
    fn the_two_decision_keys_are_different_colors() {
        let approve = action_slot(ActionRole::Approve, true, 255).color;
        let deny = action_slot(ActionRole::Deny, true, 255).color;
        assert_ne!(approve, deny, "approve and deny must never share a colour");
    }

    #[test]
    fn the_status_light_carries_the_agent_color_and_flashes_when_waiting() {
        let claude = AgentKind::Claude.brand();
        let waiting = status_slot(claude, Some(AgentState::AwaitingApproval), 255);
        assert_eq!(waiting.color, claude, "the status light is the agent's colour");
        assert_eq!(waiting.effect, Effect::Pulse, "it must flash when input is wanted");
        assert!(waiting.brightness > 0);

        assert_eq!(status_slot(claude, Some(AgentState::Working), 255).effect, Effect::Solid);
        assert_eq!(status_slot(claude, None, 255), LedSlot::OFF);
    }
}
