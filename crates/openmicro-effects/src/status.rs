//! What the board should look like, for every situation it can be in.
//!
//! This module *is* the design. Everything visual is decided here and nowhere
//! else, so there is a single answer to "what does amber mean" — and so the
//! answer can be unit-tested rather than eyeballed on hardware.
//!
//! # The board
//!
//! ```text
//!       [ 0 ][ 1 ]              agent slots — colour = which agent,
//!  [ 2 ][ 3 ][ 4 ][ 5 ]                       effect = what it is doing
//!  [ 6 ][ 7 ][ 8 ][ 9 ]         reserved, dark
//!  [ ✓ ][ ★ ][ ✗ ]              lit only while a decision is pending
//!   ◌ ◌ ◌ ◌ ◌ ◌ ◌ ◌             the ring — overall status
//! ```
//!
//! # Two independent questions
//!
//! The per-key LEDs and the ring answer different things on purpose:
//!
//! - **Keys**: *who* is busy, and with what. One key per session, in that
//!   agent's colour. Six of them, so six sessions are legible at once.
//! - **Ring**: *does this need me?* One thing, readable from across the room,
//!   which is why it gets shape-based motions rather than six more colours.
//!
//! # Why the firmware needs its own copy of this
//!
//! Two of the states below — [`Link::Offline`] and [`Link::NoDaemon`] — exist
//! precisely when the host cannot tell the device anything. If the device only
//! ever displayed what it was told, those two would both render as "dark",
//! which is also what "broken" and "flat battery" look like. So the firmware
//! runs this module locally and falls back to it whenever the host goes quiet.

use openmicro_proto::{
    ActionKeys, AgentState, Effect, Glow, LedFrame, LedSlot, Motion, Rgb, NOMINAL_SPEED,
};

use openmicro_proto::layout::{self, ActionRole, KeyRole};

/// How connected the device is.
///
/// Ordered by how much the device knows, least first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// No USB and no BLE host. The device is on its own.
    Offline,
    /// A host is there, but nothing is speaking the protocol — the daemon is
    /// not running, or has stopped answering.
    NoDaemon,
    /// The daemon is driving the display.
    Live,
}

/// Which display source the device should use, from two plain facts: is a host
/// attached, and how long since the last frame arrived.
///
/// Lives here rather than in the firmware so it is actually tested — a
/// `#[cfg(test)]` block in the firmware crate never runs, because that crate
/// only ever builds for `xtensa-esp32s3-none-elf`.
pub fn link_state(host_attached: bool, since_last_frame_ms: u32) -> Link {
    if since_last_frame_ms < openmicro_proto::DAEMON_TIMEOUT_MS {
        Link::Live
    } else if host_attached {
        Link::NoDaemon
    } else {
        Link::Offline
    }
}

/// Idle sits well below full: a calm state should not be the brightest thing on
/// the desk.
const IDLE_PCT: u32 = 45;
const THINKING_PCT: u32 = 80;
const WORKING_PCT: u32 = 100;
const AWAITING_PCT: u32 = 100;

/// Idle breathes slower than nominal; working spins faster. Same motions, read
/// as different urgencies.
const IDLE_SPEED: u8 = 96;
const THINKING_SPEED: u8 = NOMINAL_SPEED;
const WORKING_SPEED: u8 = 190;

/// "Connected, but nothing is running" — a very dim white breath. Present
/// enough to prove the whole chain works, quiet enough to ignore.
const NO_AGENTS_PCT: u32 = 18;
const NO_AGENTS_COLOR: Rgb = Rgb { r: 200, g: 220, b: 255 };

/// The colour of "a host is here but the daemon is not". Amber because it is a
/// problem you can fix, as distinct from offline, which is just a fact.
const NO_DAEMON_COLOR: Rgb = Rgb { r: 255, g: 150, b: 0 };
const NO_DAEMON_PCT: u32 = 100;

/// Offline runs [`Motion::Aurora`], which picks its own hues, so this colour is
/// unused — kept explicit rather than leaving a junk value on the wire.
const OFFLINE_COLOR: Rgb = Rgb { r: 0, g: 120, b: 200 };
const OFFLINE_PCT: u32 = 55;

fn pct(brightness: u8, percent: u32) -> u8 {
    ((brightness as u32 * percent) / 100) as u8
}

/// The ring, for a situation the device can work out by itself.
///
/// Only meaningful for [`Link::Offline`] and [`Link::NoDaemon`]; [`Link::Live`]
/// has no local answer, because when the daemon is up it is the authority on
/// what the ring shows. Callers get [`Glow::OFF`] for it and are expected to
/// use the frame the daemon sent instead.
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

/// The ring for one agent's state, in that agent's colour.
///
/// This is what the ring shows when a session has the focus: motion says what
/// is happening, colour says who it is happening to.
///
/// Takes the colour rather than an [`AgentKind`] because the user can retune
/// the palette (`AgentColors`), and this module should not have to know that.
pub fn agent_glow(color: Rgb, state: AgentState, brightness: u8) -> Glow {
    let (motion, percent, speed) = match state {
        AgentState::Idle => (Motion::Breath, IDLE_PCT, IDLE_SPEED),
        AgentState::Thinking => (Motion::Spin, THINKING_PCT, THINKING_SPEED),
        AgentState::Working => (Motion::Spin, WORKING_PCT, WORKING_SPEED),
        AgentState::AwaitingApproval => (Motion::Alert, AWAITING_PCT, NOMINAL_SPEED),
    };
    Glow { color, motion, brightness: pct(brightness, percent), speed }
}

/// The ring when the daemon is up and no session is running.
pub fn no_agents_glow(brightness: u8) -> Glow {
    Glow {
        color: NO_AGENTS_COLOR,
        motion: Motion::Breath,
        brightness: pct(brightness, NO_AGENTS_PCT),
        speed: IDLE_SPEED,
    }
}

/// The per-key effect for an agent state.
///
/// Deliberately quieter than the ring's vocabulary: with up to six of these lit
/// at once, six competing animations would be unreadable. Only the state that
/// actually wants a press gets movement.
pub fn slot_effect(state: AgentState) -> Effect {
    match state {
        // Idle is barely there — the session exists, nothing more.
        AgentState::Idle => Effect::Breath,
        AgentState::Thinking => Effect::Breath,
        AgentState::Working => Effect::Solid,
        // The one per-key animation, so a waiting session is findable among
        // five busy ones.
        AgentState::AwaitingApproval => Effect::Pulse,
    }
}

/// The key for an agent slot: agent colour, state effect.
pub fn slot_for(color: Rgb, state: AgentState, brightness: u8) -> LedSlot {
    let percent = match state {
        AgentState::Idle => IDLE_PCT,
        AgentState::Thinking => THINKING_PCT,
        AgentState::Working => WORKING_PCT,
        AgentState::AwaitingApproval => AWAITING_PCT,
    };
    LedSlot { color, effect: slot_effect(state), brightness: pct(brightness, percent) }
}

/// One bottom-row key.
///
/// Held [`Effect::Solid`] rather than pulsing. These are targets to hit, and a
/// moving target reads as a warning; the ring's [`Motion::Alert`] is already
/// carrying the urgency, and two things shouting at once is worse than one.
/// Their simply *appearing* is the signal that something changed.
pub fn action_slot(role: ActionRole, armed: bool, brightness: u8) -> LedSlot {
    if !armed {
        return LedSlot::OFF;
    }
    LedSlot { color: role.color(), effect: Effect::Solid, brightness }
}

/// The stop key on row 2.
///
/// Kept dimmer than the decision row: it is a standing option, not a reply to a
/// question, and it should not compete with a live prompt for attention.
const INTERRUPT_PCT: u32 = 55;

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

/// Expand a frame into every key's slot, indexed by key id.
///
/// Lives here, rather than on either side, because both ends need it: the
/// firmware to paint the chain, and the daemon (and TUI preview) to reason about
/// the whole board. Two implementations of this would be two chances for the key
/// that lights up and the key that acts to disagree.
///
/// Note this is *key* order, not LED-chain order — mapping one to the other is
/// `layout::LED_FOR_KEY`, and it is the firmware's job.
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
            KeyRole::Always => action_slot(ActionRole::Always, frame.actions.always, b),
            KeyRole::Deny => action_slot(ActionRole::Deny, frame.actions.deny, b),
        };
    }
    keys
}

/// Which keys to offer for a state.
///
/// A lit button that does nothing trains you to stop believing the lights, so
/// each key appears exactly when pressing it would have an effect:
/// - the bottom row only while a request is actually pending;
/// - the stop key only while there is work in progress to stop.
pub fn action_keys_for(state: Option<AgentState>) -> ActionKeys {
    match state {
        Some(AgentState::AwaitingApproval) => {
            // Nothing is executing while the agent waits, so there is nothing
            // to interrupt.
            ActionKeys { approve: true, always: true, deny: true, interrupt: false }
        }
        Some(AgentState::Thinking) | Some(AgentState::Working) => {
            ActionKeys { interrupt: true, ..ActionKeys::NONE }
        }
        // Idle or nothing selected: no key would do anything.
        _ => ActionKeys::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmicro_proto::AgentKind;

    /// Test shims: the real functions take a colour, because the palette is
    /// user-tunable. These tests are about the *mapping*, so they go via the
    /// brand colour to keep the intent readable.
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
        // Still live after a couple of missed heartbeats — the display must not
        // flicker between modes because one BLE write was dropped.
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
        // Frames over BLE while running on battery: USB-detect reads low, but
        // something is plainly talking to us, so believe the frames.
        assert_eq!(link_state(false, 0), Link::Live);
    }

    #[test]
    fn offline_and_no_daemon_never_look_the_same() {
        // The whole reason both exist is to be told apart. Different motion
        // *and* different colour, so neither channel alone has to carry it.
        let off = local_glow(Link::Offline, 255);
        let nod = local_glow(Link::NoDaemon, 255);
        assert_ne!(off.motion, nod.motion);
        assert_ne!(off.color, nod.color);
    }

    #[test]
    fn neither_offline_state_is_dark() {
        // Dark is indistinguishable from broken, flat, or unplugged. Both of
        // these states must visibly show something.
        for link in [Link::Offline, Link::NoDaemon] {
            let g = local_glow(link, 255);
            assert!(g.brightness > 0, "{link:?} rendered dark");
            assert_ne!(g.motion, Motion::Off, "{link:?} rendered as Off");
        }
    }

    #[test]
    fn live_has_no_local_answer() {
        // When the daemon is up it is the authority; a local guess here would
        // fight the frame the daemon sent.
        assert_eq!(local_glow(Link::Live, 255), Glow::OFF);
    }

    #[test]
    fn every_agent_state_gets_a_distinct_looking_ring() {
        // Two states that render identically are two states the user cannot
        // distinguish, which makes one of them pointless.
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
        // This is the user-visible contract: circles mean running, and the
        // attention-getting flash is reserved for needing input.
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
        // "Nothing to report" must not compete with "something is happening".
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
        // The change from the old design: colour identifies *who*, and the
        // effect carries *what*. A state must never override the colour.
        for state in ALL_STATES {
            assert_eq!(sl(AgentKind::Claude, state, 255).color, AgentKind::Claude.brand());
            assert_eq!(sl(AgentKind::Grok, state, 255).color, AgentKind::Grok.brand());
        }
        // ...and two agents in the same state are still told apart.
        let a = sl(AgentKind::Claude, AgentState::Working, 255);
        let b = sl(AgentKind::Codex, AgentState::Working, 255);
        assert_ne!(a.color, b.color);
        assert_eq!(a.effect, b.effect);
    }

    #[test]
    fn only_the_waiting_state_animates_a_key() {
        // Six pulsing keys would be unreadable; exactly one is findable.
        assert_eq!(slot_effect(AgentState::AwaitingApproval), Effect::Pulse);
        for state in [AgentState::Working, AgentState::Idle, AgentState::Thinking] {
            assert_ne!(slot_effect(state), Effect::Pulse, "{state:?} must not pulse");
        }
    }

    #[test]
    fn action_keys_appear_only_when_a_decision_is_pending() {
        let armed = action_keys_for(Some(AgentState::AwaitingApproval));
        assert!(armed.approve && armed.always && armed.deny);

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
        // Waiting for a decision means nothing is executing, so there is
        // nothing to stop — and a lit stop key next to a lit deny key would be
        // needlessly ambiguous.
        assert!(!action_keys_for(Some(AgentState::AwaitingApproval)).interrupt);
        assert!(!action_keys_for(Some(AgentState::Idle)).interrupt);
        assert!(!action_keys_for(None).interrupt);
    }

    #[test]
    fn stop_and_deny_never_light_at_the_same_time() {
        // They share a hue, so overlapping would be genuinely confusing.
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
        // `any()` gates "is something waiting on me?", so the stop key being
        // lit must not make it true.
        let stopping = action_keys_for(Some(AgentState::Working));
        assert!(stopping.interrupt);
        assert!(!stopping.any(), "a stoppable session is not a pending decision");
    }

    #[test]
    fn an_unarmed_action_key_is_fully_off() {
        for role in [ActionRole::Approve, ActionRole::Always, ActionRole::Deny] {
            assert_eq!(action_slot(role, false, 255), LedSlot::OFF, "{role:?}");
        }
    }

    #[test]
    fn armed_action_keys_are_steady_and_correctly_colored() {
        let approve = action_slot(ActionRole::Approve, true, 255);
        let deny = action_slot(ActionRole::Deny, true, 255);
        assert_eq!(approve.color, openmicro_proto::APPROVE_COLOR);
        assert_eq!(deny.color, openmicro_proto::DENY_COLOR);
        // Steady, not pulsing: the ring does the shouting.
        assert_eq!(approve.effect, Effect::Solid);
        assert_eq!(deny.effect, Effect::Solid);
        assert!(approve.brightness > 0 && deny.brightness > 0);
    }

    #[test]
    fn the_three_action_keys_are_three_different_colors() {
        let colors = [
            action_slot(ActionRole::Approve, true, 255).color,
            action_slot(ActionRole::Always, true, 255).color,
            action_slot(ActionRole::Deny, true, 255).color,
        ];
        for (i, a) in colors.iter().enumerate() {
            for b in colors.iter().skip(i + 1) {
                assert_ne!(a, b, "two action keys share a colour");
            }
        }
    }
}
