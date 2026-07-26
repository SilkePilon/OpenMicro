pub const HOLD_OFF_MS: u32 = 2_000;

pub const HOLD_BOOTLOADER_MS: u32 = 8_000;

pub const DEBOUNCE_MS: u32 = 30;

const _: () = assert!(
    HOLD_BOOTLOADER_MS >= HOLD_OFF_MS * 3,
    "the power-off and bootloader holds are too close together"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    None,
    Wake,
    PowerOff,
    EnterBootloader,
}

#[derive(Debug, Default)]
pub struct PowerButton {
    down_since: Option<u32>,
    fired: bool,
}

impl PowerButton {
    pub fn new() -> PowerButton {
        PowerButton::default()
    }

    pub fn update(&mut self, pressed: bool, now_ms: u32) -> PowerAction {
        match (self.down_since, pressed) {
            (None, true) => {
                self.down_since = Some(now_ms);
                self.fired = false;
                PowerAction::None
            }
            (Some(start), true) => {
                if self.fired {
                    return PowerAction::None;
                }
                let held = now_ms.wrapping_sub(start);
                if held >= HOLD_BOOTLOADER_MS {
                    self.fired = true;
                    PowerAction::EnterBootloader
                } else if held >= HOLD_OFF_MS {
                    self.fired = false;
                    PowerAction::PowerOff
                } else {
                    PowerAction::None
                }
            }
            (Some(start), false) => {
                let held = now_ms.wrapping_sub(start);
                self.down_since = None;
                let fired = self.fired;
                self.fired = false;
                if fired || held >= HOLD_OFF_MS {
                    PowerAction::None
                } else if held >= DEBOUNCE_MS {
                    PowerAction::Wake
                } else {
                    PowerAction::None
                }
            }
            (None, false) => PowerAction::None,
        }
    }

    pub fn is_down(&self) -> bool {
        self.down_since.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press_for(hold_ms: u32) -> std::vec::Vec<PowerAction> {
        let mut b = PowerButton::new();
        let mut out = std::vec::Vec::new();
        let mut t = 0;
        while t <= hold_ms {
            let a = b.update(true, t);
            if a != PowerAction::None {
                out.push(a);
            }
            t += 10;
        }
        let a = b.update(false, hold_ms);
        if a != PowerAction::None {
            out.push(a);
        }
        out
    }

    #[test]
    fn a_short_press_wakes() {
        assert_eq!(press_for(120), std::vec![PowerAction::Wake]);
    }

    #[test]
    fn a_bounce_is_ignored() {
        assert!(press_for(10).is_empty(), "a 10ms blip is contact bounce");
        assert!(press_for(DEBOUNCE_MS - 1).is_empty());
    }

    #[test]
    fn a_two_second_hold_powers_off() {
        let actions = press_for(HOLD_OFF_MS + 100);
        assert!(actions.contains(&PowerAction::PowerOff), "{actions:?}");
        assert!(!actions.contains(&PowerAction::Wake), "a hold is not also a press");
    }

    #[test]
    fn power_off_fires_while_held_not_on_release() {
        let mut b = PowerButton::new();
        b.update(true, 0);
        assert_eq!(b.update(true, HOLD_OFF_MS - 1), PowerAction::None);
        assert_eq!(b.update(true, HOLD_OFF_MS), PowerAction::PowerOff);
    }

    #[test]
    fn a_long_hold_reaches_the_bootloader_and_stops_there() {
        let actions = press_for(HOLD_BOOTLOADER_MS + 200);
        assert!(actions.contains(&PowerAction::EnterBootloader), "{actions:?}");
        let after: std::vec::Vec<_> = actions
            .iter()
            .skip_while(|a| **a != PowerAction::EnterBootloader)
            .skip(1)
            .collect();
        assert!(after.is_empty(), "kept firing after the bootloader: {after:?}");
    }

    #[test]
    fn releasing_after_a_hold_does_not_also_wake() {
        let mut b = PowerButton::new();
        b.update(true, 0);
        assert_eq!(b.update(true, HOLD_OFF_MS), PowerAction::PowerOff);
        assert_eq!(b.update(false, HOLD_OFF_MS + 50), PowerAction::None);
    }

    #[test]
    fn presses_are_independent() {
        let mut b = PowerButton::new();
        b.update(true, 0);
        assert_eq!(b.update(false, 100), PowerAction::Wake);
        b.update(true, 200);
        assert_eq!(b.update(false, 300), PowerAction::Wake, "second press still works");
    }

    #[test]
    fn is_down_tracks_the_button() {
        let mut b = PowerButton::new();
        assert!(!b.is_down());
        b.update(true, 0);
        assert!(b.is_down());
        b.update(false, 100);
        assert!(!b.is_down());
    }

    #[test]
    fn a_wrapping_clock_does_not_produce_a_spurious_hold() {
        let mut b = PowerButton::new();
        b.update(true, u32::MAX - 50);
        assert_eq!(b.update(true, u32::MAX - 10), PowerAction::None);
        assert_eq!(b.update(false, 10u32.wrapping_sub(0)), PowerAction::Wake);
    }
}
