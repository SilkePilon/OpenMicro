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

/// Which agent occupies a slot, so the key can be lit in that agent's colour.
///
/// State alone answers "is something busy?"; the colour answers "who?", which
/// is the question that matters when three sessions are running at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentKind {
    Claude,
    Codex,
    Grok,
    /// Anything the adapters do not recognise. Deliberately not folded into
    /// one of the above: a mystery agent should look like a mystery, not
    /// impersonate Claude.
    Other,
}

impl AgentKind {
    /// Match the agent name the hooks report (`claude`, `codex`, `grok`).
    pub fn from_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case("claude") {
            Self::Claude
        } else if name.eq_ignore_ascii_case("codex") {
            Self::Codex
        } else if name.eq_ignore_ascii_case("grok") {
            Self::Grok
        } else {
            Self::Other
        }
    }

    /// The agent's colour on the keypad.
    ///
    /// Chosen to be unmistakable from each other across a dim room at a
    /// glance, which matters more here than matching a brand hex exactly —
    /// these are 5 mm diffused emitters seen through a translucent keycap, not
    /// a screen. White is held slightly below full scale because a saturated
    /// WS2812 white next to a coloured neighbour swamps it.
    pub const fn brand(self) -> Rgb {
        match self {
            Self::Claude => Rgb { r: 255, g: 120, b: 30 },
            Self::Codex => Rgb { r: 230, g: 230, b: 230 },
            Self::Grok => Rgb { r: 160, g: 60, b: 255 },
            Self::Other => Rgb { r: 120, g: 120, b: 120 },
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
            Self::Other => "agent",
        }
    }
}

/// How the underglow ring moves.
///
/// The ring is the device's status line, and these are its words. Each one has
/// a deliberately different *shape* — swell, travel, flash — not just a
/// different speed, because speed alone is hard to read at a glance while two
/// of them are on screen at different times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Motion {
    /// Dark.
    Off,
    /// The whole ring swells and fades together. Calm: nothing is happening.
    Breath,
    /// A comet travels around the ring. Something is running.
    Spin,
    /// Sharp double-blink of the whole ring. A decision is waiting on you.
    Alert,
    /// Slow multi-hue drift. No host at all — the device is on its own.
    Aurora,
    /// One dim dot orbiting slowly. A host is there but the daemon is not.
    Searching,
}

/// The underglow ring's state.
///
/// Sent as *intent*, not pixels. A `Spin` at 60 fps would be ~1.4 kB/s of BLE
/// writes to say one word, and it would stutter on every dropped frame; worse,
/// it could not run at all when the daemon is the thing that has gone away.
/// The firmware owns the animation, the host owns the meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Glow {
    pub color: Rgb,
    pub motion: Motion,
    pub brightness: u8,
    /// Rate, where 128 is the motion's nominal speed and 255 is twice it.
    /// Lets Thinking and Working share `Spin` and still be distinguishable.
    pub speed: u8,
}

impl Glow {
    pub const OFF: Glow = Glow {
        color: Rgb { r: 0, g: 0, b: 0 },
        motion: Motion::Off,
        brightness: 0,
        speed: NOMINAL_SPEED,
    };
}

/// The neutral value of [`Glow::speed`].
pub const NOMINAL_SPEED: u8 = 128;

/// How often the daemon resends the current frame even when nothing changed.
///
/// Without this, silence is ambiguous: a daemon with nothing new to say looks
/// exactly like one that has died, and the device cannot know whether to keep
/// showing the last frame or fall back to its own "no daemon" animation.
pub const HEARTBEAT_MS: u64 = 1500;

/// How long the device waits before deciding the daemon is gone.
///
/// Comfortably more than three heartbeats, so a couple of dropped BLE writes do
/// not make the display flicker between live and disconnected.
pub const DAEMON_TIMEOUT_MS: u32 = 6000;

const _: () = assert!(
    DAEMON_TIMEOUT_MS as u64 > HEARTBEAT_MS * 3,
    "the timeout must tolerate several missed heartbeats"
);

/// Which of the three bottom-row keys are live.
///
/// They light *only* when a press would do something. An always-lit button
/// that usually does nothing teaches you to ignore it, which is the opposite
/// of what an approval prompt needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ActionKeys {
    pub approve: bool,
    pub always: bool,
    pub deny: bool,
    /// Row 2's stop key. Offered whenever there is work to stop, which is a
    /// different condition from the three above — hence a separate flag rather
    /// than folding it into the decision row.
    pub interrupt: bool,
}

impl ActionKeys {
    pub const NONE: ActionKeys =
        ActionKeys { approve: false, always: false, deny: false, interrupt: false };

    /// True when a decision is being offered — i.e. the bottom row is visible.
    ///
    /// Deliberately excludes `interrupt`: that key is about ongoing work, not a
    /// pending question, and callers asking "is something waiting on me?" would
    /// get the wrong answer if it counted.
    pub const fn any(&self) -> bool {
        self.approve || self.always || self.deny
    }
}

/// Dim red for stop. Shares a hue with [`DENY_COLOR`] because both are the
/// negative answer, and sits at a lower brightness because it is a
/// standing option rather than a reply to a question.
pub const INTERRUPT_COLOR: Rgb = Rgb { r: 180, g: 25, b: 15 };

/// Green for yes.
pub const APPROVE_COLOR: Rgb = Rgb { r: 0, g: 230, b: 70 };
/// Amber for "yes, and stop asking" — kin to approve, but not the same
/// decision, so not the same colour.
pub const ALWAYS_COLOR: Rgb = Rgb { r: 255, g: 170, b: 0 };
/// Red for no.
pub const DENY_COLOR: Rgb = Rgb { r: 255, g: 30, b: 20 };

/// Per-agent key colours, overridable by the user.
///
/// Replaces the older per-*state* palette. Colour now answers "which agent",
/// and the effect answers "doing what" — one channel per question, so both stay
/// readable when six sessions are lit at once. Colouring by state instead meant
/// three simultaneous Claude sessions were indistinguishable, which is the case
/// that actually needs telling apart.
///
/// `#[serde(default)]` on the struct is deliberate: a config written by an older
/// build has `idle`/`thinking`/... under this key, and defaulting each field
/// lets the rest of that file survive being read instead of taking the whole
/// config down with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentColors {
    pub claude: Rgb,
    pub codex: Rgb,
    pub grok: Rgb,
    pub other: Rgb,
}

impl Default for AgentColors {
    fn default() -> Self {
        Self {
            claude: AgentKind::Claude.brand(),
            codex: AgentKind::Codex.brand(),
            grok: AgentKind::Grok.brand(),
            other: AgentKind::Other.brand(),
        }
    }
}

impl AgentColors {
    pub fn for_kind(&self, kind: AgentKind) -> Rgb {
        match kind {
            AgentKind::Claude => self.claude,
            AgentKind::Codex => self.codex,
            AgentKind::Grok => self.grok,
            AgentKind::Other => self.other,
        }
    }

    pub fn set(&mut self, kind: AgentKind, rgb: Rgb) {
        match kind {
            AgentKind::Claude => self.claude = rgb,
            AgentKind::Codex => self.codex = rgb,
            AgentKind::Grok => self.grok = rgb,
            AgentKind::Other => self.other = rgb,
        }
    }
}

/// Commands sent from the TUI to the daemon over the control socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    SetBrightness(u8),
    SetAgentColor { agent: AgentKind, rgb: Rgb },
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
    /// One entry per agent key (top two rows).
    pub slots: [LedSlot; SLOT_COUNT],
    /// The underglow ring.
    pub glow: Glow,
    /// Which action keys are offered.
    pub actions: ActionKeys,
    /// Master brightness.
    ///
    /// Carried explicitly because the action keys are sent as booleans, not
    /// pixels — the device knows their colours, but not how bright the user
    /// wants them, and reading it back off an agent slot would fail whenever
    /// no session happens to be lit.
    pub brightness: u8,
}

impl LedFrame {
    pub const BLANK: LedFrame = LedFrame {
        slots: [LedSlot::OFF; SLOT_COUNT],
        glow: Glow::OFF,
        actions: ActionKeys::NONE,
        brightness: 0,
    };

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
        frame.glow = Glow {
            color: Rgb { r: 1, g: 2, b: 3 },
            motion: Motion::Spin,
            brightness: 77,
            speed: 200,
        };
        frame.actions = ActionKeys { approve: true, always: false, deny: true, interrupt: false };
        let bytes = frame.encode().unwrap();
        let back = LedFrame::decode(&bytes).unwrap();
        assert_eq!(frame, back, "glow and action keys must survive the wire");
    }

    #[test]
    fn agent_names_map_to_kinds_case_insensitively() {
        assert_eq!(AgentKind::from_name("claude"), AgentKind::Claude);
        assert_eq!(AgentKind::from_name("Claude"), AgentKind::Claude);
        assert_eq!(AgentKind::from_name("codex"), AgentKind::Codex);
        assert_eq!(AgentKind::from_name("grok"), AgentKind::Grok);
        // An unknown agent stays unknown rather than defaulting to Claude.
        assert_eq!(AgentKind::from_name("aider"), AgentKind::Other);
        assert_eq!(AgentKind::from_name(""), AgentKind::Other);
    }

    #[test]
    fn brand_colors_are_the_ones_documented() {
        assert_eq!(AgentKind::Claude.brand(), Rgb { r: 255, g: 120, b: 30 }, "orange");
        assert_eq!(AgentKind::Codex.brand(), Rgb { r: 230, g: 230, b: 230 }, "white");
        assert_eq!(AgentKind::Grok.brand(), Rgb { r: 160, g: 60, b: 255 }, "purple");
    }

    #[test]
    fn every_agent_color_is_distinguishable_from_the_others() {
        // The whole point of brand colours is telling sessions apart at a
        // glance. Two agents within a few units of each other would defeat it,
        // so assert a real separation rather than mere inequality.
        let kinds = [AgentKind::Claude, AgentKind::Codex, AgentKind::Grok, AgentKind::Other];
        for (i, a) in kinds.iter().enumerate() {
            for b in kinds.iter().skip(i + 1) {
                let (x, y) = (a.brand(), b.brand());
                let spread = (x.r as i32 - y.r as i32).abs()
                    + (x.g as i32 - y.g as i32).abs()
                    + (x.b as i32 - y.b as i32).abs();
                assert!(spread > 120, "{:?} and {:?} look too alike ({spread})", a, b);
            }
        }
    }

    #[test]
    fn action_key_colors_read_as_go_and_stop() {
        // Green must be dominantly green and red dominantly red: a "deny" key
        // that leans green is actively dangerous.
        assert!(APPROVE_COLOR.g > APPROVE_COLOR.r + 100);
        assert!(DENY_COLOR.r > DENY_COLOR.g + 100);
        // Amber sits between them, and is not mistakable for either.
        assert!(ALWAYS_COLOR.r > 200 && ALWAYS_COLOR.g > 100 && ALWAYS_COLOR.b < 60);
    }

    #[test]
    fn action_keys_any_tracks_the_individual_flags() {
        assert!(!ActionKeys::NONE.any());
        assert!(ActionKeys { approve: true, ..Default::default() }.any());
        assert!(ActionKeys { deny: true, ..Default::default() }.any());
        assert!(ActionKeys { always: true, ..Default::default() }.any());
    }

    #[test]
    fn a_blank_frame_shows_nothing_at_all() {
        let blank = LedFrame::BLANK;
        assert!(!blank.actions.any());
        assert_eq!(blank.glow.motion, Motion::Off);
        assert_eq!(blank.glow.brightness, 0);
        for slot in blank.slots {
            assert_eq!(slot, LedSlot::OFF);
        }
    }

    #[test]
    fn agent_colors_default_to_the_brand_palette() {
        let c = AgentColors::default();
        for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Grok, AgentKind::Other] {
            assert_eq!(c.for_kind(kind), kind.brand(), "{kind:?}");
        }
    }

    #[test]
    fn setting_one_agent_color_leaves_the_others_alone() {
        let mut c = AgentColors::default();
        c.set(AgentKind::Grok, Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(c.for_kind(AgentKind::Grok), Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(c.for_kind(AgentKind::Claude), AgentKind::Claude.brand());
        assert_eq!(c.for_kind(AgentKind::Codex), AgentKind::Codex.brand());
    }

    #[test]
    fn an_older_configs_color_table_does_not_break_parsing() {
        // Configs written before colour meant "which agent" carry per-state
        // keys here. They must degrade to the defaults, not fail the read and
        // take the user's brightness with them.
        let old = r#"{"idle":{"r":0,"g":0,"b":0},"working":{"r":0,"g":200,"b":80}}"#;
        let parsed: AgentColors = serde_json::from_str(old).expect("old shape must still parse");
        assert_eq!(parsed, AgentColors::default());
    }

    #[test]
    fn command_json_roundtrips() {
        let cmd = Command::SetAgentColor {
            agent: AgentKind::Claude,
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
