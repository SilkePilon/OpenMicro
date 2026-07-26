//! A scripted walk through every state the board can show.
//!
//! # Why this exists
//!
//! The BLE GATT server is not wired yet, so on real hardware the device can only
//! ever reach [`Link::NoDaemon`] and [`Link::Offline`] — the two states it works
//! out for itself. Everything the *host* drives (agent colours, the spinner, the
//! decision row) is implemented and unit-tested but has no way to arrive.
//!
//! This module fabricates those frames locally so the design can be seen on the
//! device before the link exists. It is a demo, not a fallback: nothing selects
//! it automatically, because a board inventing agent states it cannot know would
//! be actively misleading.
//!
//! It lives here rather than in the firmware so the script itself is testable —
//! and because building the frames means going through `status`, which is the
//! point: if the demo looks right, the real path renders the same way.

use openmicro_proto::{AgentColors, AgentKind, AgentState, LedFrame, SLOT_COUNT};

use crate::status::{self, Link};

/// How long each scene holds. Long enough to take in a slow `Breath` swell
/// (3.4 s nominal) without the walk becoming tedious.
pub const SCENE_MS: u32 = 4000;

/// Brightness the demo renders at.
pub const DEMO_BRIGHTNESS: u8 = 150;

/// What the board is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    /// A state the device works out for itself; the ring animates, keys dark.
    Local(Link),
    /// A frame as the daemon would have sent it.
    Host(LedFrame),
}

/// One step of the walk, with a label for the serial log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub label: &'static str,
    pub scene: Scene,
}

/// Number of scenes in the script.
pub const SCENE_COUNT: usize = 8;

fn one(kind: AgentKind, state: AgentState) -> LedFrame {
    many(&[(kind, state)], 0)
}

/// A frame with `sessions` occupying slots 0.., focused on `focus`.
fn many(sessions: &[(AgentKind, AgentState)], focus: usize) -> LedFrame {
    let colors = AgentColors::default();
    let mut frame = LedFrame::BLANK;
    frame.brightness = DEMO_BRIGHTNESS;
    for (i, (kind, state)) in sessions.iter().enumerate() {
        if i < SLOT_COUNT {
            frame.slots[i] =
                status::slot_for(colors.for_kind(*kind), *state, DEMO_BRIGHTNESS);
        }
    }
    let (kind, state) = sessions[focus.min(sessions.len() - 1)];
    frame.glow = status::agent_glow(colors.for_kind(kind), state, DEMO_BRIGHTNESS);
    frame.actions = status::action_keys_for(Some(state));
    frame
}

/// The `index`th scene, wrapping.
pub fn scene(index: usize) -> Step {
    match index % SCENE_COUNT {
        0 => Step { label: "offline — no host at all", scene: Scene::Local(Link::Offline) },
        1 => Step {
            label: "host up, daemon down",
            scene: Scene::Local(Link::NoDaemon),
        },
        2 => Step {
            label: "daemon up, nothing running",
            scene: Scene::Host({
                let mut f = LedFrame::BLANK;
                f.brightness = DEMO_BRIGHTNESS;
                f.glow = status::no_agents_glow(DEMO_BRIGHTNESS);
                f
            }),
        },
        3 => Step {
            label: "Claude thinking — orange, slow spin",
            scene: Scene::Host(one(AgentKind::Claude, AgentState::Thinking)),
        },
        4 => Step {
            label: "Codex working — white, fast spin, stop key lit",
            scene: Scene::Host(one(AgentKind::Codex, AgentState::Working)),
        },
        5 => Step {
            label: "Grok working — purple",
            scene: Scene::Host(one(AgentKind::Grok, AgentState::Working)),
        },
        6 => Step {
            label: "three agents at once, focused on Claude",
            scene: Scene::Host(many(
                &[
                    (AgentKind::Claude, AgentState::Working),
                    (AgentKind::Codex, AgentState::Thinking),
                    (AgentKind::Grok, AgentState::Idle),
                ],
                0,
            )),
        },
        _ => Step {
            label: "Claude waiting — ring alerts, approve/always/deny lit",
            scene: Scene::Host(one(AgentKind::Claude, AgentState::AwaitingApproval)),
        },
    }
}

/// The scene index at `t_ms`, and the scene.
pub fn scene_at(t_ms: u32) -> (usize, Step) {
    let index = (t_ms / SCENE_MS) as usize % SCENE_COUNT;
    (index, scene(index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmicro_proto::Motion;

    #[test]
    fn the_script_covers_every_link_state_and_every_agent_state() {
        // The demo's whole job is to show the design, so anything the design can
        // express must appear somewhere in the walk — otherwise it ships
        // unverified by eye.
        let mut motions = std::vec::Vec::new();
        let mut colors = std::vec::Vec::new();
        let mut saw_actions = false;
        let mut saw_interrupt = false;
        for i in 0..SCENE_COUNT {
            match scene(i).scene {
                Scene::Local(link) => {
                    motions.push(status::local_glow(link, DEMO_BRIGHTNESS).motion)
                }
                Scene::Host(f) => {
                    motions.push(f.glow.motion);
                    colors.push(f.glow.color);
                    saw_actions |= f.actions.any();
                    saw_interrupt |= f.actions.interrupt;
                }
            }
        }
        for expected in
            [Motion::Aurora, Motion::Searching, Motion::Breath, Motion::Spin, Motion::Alert]
        {
            assert!(motions.contains(&expected), "the walk never shows {expected:?}");
        }
        for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Grok] {
            assert!(colors.contains(&kind.brand()), "the walk never shows {kind:?}");
        }
        assert!(saw_actions, "the walk never lights the decision row");
        assert!(saw_interrupt, "the walk never lights the stop key");
    }

    #[test]
    fn scenes_advance_over_time_and_wrap() {
        assert_eq!(scene_at(0).0, 0);
        assert_eq!(scene_at(SCENE_MS).0, 1);
        assert_eq!(scene_at(SCENE_MS * (SCENE_COUNT as u32)).0, 0, "the walk loops");
    }

    #[test]
    fn the_index_is_always_in_range() {
        // `t_ms` is a wrapping clock, so nothing may index off the end.
        for t in [0u32, 1, SCENE_MS - 1, u32::MAX, u32::MAX - 1] {
            let (i, _) = scene_at(t);
            assert!(i < SCENE_COUNT, "index {i} out of range at t={t}");
        }
        let _ = scene(usize::MAX);
    }

    #[test]
    fn a_multi_agent_scene_lights_one_key_per_agent_in_its_own_color() {
        let Scene::Host(f) = scene(6).scene else { panic!("scene 6 should be a host frame") };
        assert_eq!(f.slots[0].color, AgentKind::Claude.brand());
        assert_eq!(f.slots[1].color, AgentKind::Codex.brand());
        assert_eq!(f.slots[2].color, AgentKind::Grok.brand());
        // ...and the ring follows the focused one, not an average of them.
        assert_eq!(f.glow.color, AgentKind::Claude.brand());
    }

    #[test]
    fn every_host_scene_carries_a_usable_brightness() {
        // A frame with brightness 0 would render the action keys invisible and
        // the demo would look broken rather than empty.
        for i in 0..SCENE_COUNT {
            if let Scene::Host(f) = scene(i).scene {
                assert!(f.brightness > 0, "scene {i} has zero brightness");
            }
        }
    }

    #[test]
    fn the_waiting_scene_lights_all_three_decision_keys() {
        let Scene::Host(f) = scene(7).scene else { panic!("scene 7 should be a host frame") };
        assert!(f.actions.approve && f.actions.always && f.actions.deny);
        assert_eq!(f.glow.motion, Motion::Alert);
    }
}
