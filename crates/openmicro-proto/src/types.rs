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
pub enum AgentKind {
    Claude,
    Codex,
    Grok,
    Opencode,
    Other,
}

impl AgentKind {
    pub fn from_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case("claude") {
            Self::Claude
        } else if name.eq_ignore_ascii_case("codex") {
            Self::Codex
        } else if name.eq_ignore_ascii_case("grok") {
            Self::Grok
        } else if name.eq_ignore_ascii_case("opencode") {
            Self::Opencode
        } else {
            Self::Other
        }
    }

    pub const fn brand(self) -> Rgb {
        match self {
            Self::Claude => Rgb { r: 255, g: 120, b: 30 },
            Self::Codex => Rgb { r: 230, g: 230, b: 230 },
            Self::Grok => Rgb { r: 160, g: 60, b: 255 },
            Self::Opencode => Rgb { r: 0, g: 200, b: 190 },
            Self::Other => Rgb { r: 120, g: 120, b: 120 },
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
            Self::Opencode => "opencode",
            Self::Other => "agent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Motion {
    Off,
    Breath,
    Spin,
    Alert,
    Aurora,
    Searching,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Glow {
    pub color: Rgb,
    pub motion: Motion,
    pub brightness: u8,
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

pub const NOMINAL_SPEED: u8 = 128;

pub const HEARTBEAT_MS: u64 = 1500;

pub const DAEMON_TIMEOUT_MS: u32 = 6000;

const _: () = assert!(
    DAEMON_TIMEOUT_MS as u64 > HEARTBEAT_MS * 3,
    "the timeout must tolerate several missed heartbeats"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ActionKeys {
    pub approve: bool,
    pub deny: bool,
    pub interrupt: bool,
}

impl ActionKeys {
    pub const NONE: ActionKeys = ActionKeys { approve: false, deny: false, interrupt: false };

    pub const fn any(&self) -> bool {
        self.approve || self.deny
    }
}

pub const INTERRUPT_COLOR: Rgb = Rgb { r: 180, g: 25, b: 15 };

pub const APPROVE_COLOR: Rgb = Rgb { r: 0, g: 230, b: 70 };
pub const DENY_COLOR: Rgb = Rgb { r: 255, g: 30, b: 20 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentColors {
    pub claude: Rgb,
    pub codex: Rgb,
    pub grok: Rgb,
    pub opencode: Rgb,
    pub other: Rgb,
}

impl Default for AgentColors {
    fn default() -> Self {
        Self {
            claude: AgentKind::Claude.brand(),
            codex: AgentKind::Codex.brand(),
            grok: AgentKind::Grok.brand(),
            opencode: AgentKind::Opencode.brand(),
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
            AgentKind::Opencode => self.opencode,
            AgentKind::Other => self.other,
        }
    }

    pub fn set(&mut self, kind: AgentKind, rgb: Rgb) {
        match kind {
            AgentKind::Claude => self.claude = rgb,
            AgentKind::Codex => self.codex = rgb,
            AgentKind::Grok => self.grok = rgb,
            AgentKind::Opencode => self.opencode = rgb,
            AgentKind::Other => self.other = rgb,
        }
    }
}

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
    pub slots: [LedSlot; SLOT_COUNT],
    pub glow: Glow,
    pub actions: ActionKeys,
    pub status: LedSlot,
    pub brightness: u8,
}

impl LedFrame {
    pub const BLANK: LedFrame = LedFrame {
        slots: [LedSlot::OFF; SLOT_COUNT],
        glow: Glow::OFF,
        actions: ActionKeys::NONE,
        status: LedSlot::OFF,
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
        frame.actions = ActionKeys { approve: true, deny: true, interrupt: false };
        frame.status = LedSlot {
            color: Rgb { r: 9, g: 9, b: 9 },
            effect: Effect::Pulse,
            brightness: 100,
        };
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
        let kinds = [
            AgentKind::Claude,
            AgentKind::Codex,
            AgentKind::Grok,
            AgentKind::Opencode,
            AgentKind::Other,
        ];
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
        assert!(APPROVE_COLOR.g > APPROVE_COLOR.r + 100);
        assert!(DENY_COLOR.r > DENY_COLOR.g + 100);
    }

    #[test]
    fn action_keys_any_tracks_the_individual_flags() {
        assert!(!ActionKeys::NONE.any());
        assert!(ActionKeys { approve: true, ..Default::default() }.any());
        assert!(ActionKeys { deny: true, ..Default::default() }.any());
        assert!(!ActionKeys { interrupt: true, ..Default::default() }.any());
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
        assert_eq!(blank.status, LedSlot::OFF);
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
