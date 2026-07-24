//! Power-button behaviour.
//!
//! One button, three meanings, told apart by how long it is held:
//!
//! | Gesture | Result |
//! |---|---|
//! | short press | wake / turn the lights on |
//! | hold ~2 s | power off |
//! | hold ~8 s | reboot into the ROM bootloader |
//!
//! The long hold exists as an escape hatch: the host can ask the firmware to
//! enter download mode over BLE, but if the firmware is wedged badly enough
//! that BLE is not answering, that route is gone. A button hold is handled far
//! enough down the stack to still work when most of the firmware does not.
//!
//! Pure state machine over `(pressed, now_ms)`, so the timing — the part that
//! is miserable to debug on a device with no display — is unit-tested here
//! rather than discovered on hardware.

/// How long the button must be held before it means "off".
pub const HOLD_OFF_MS: u32 = 2_000;

/// How long it must be held before it means "bootloader". Deliberately far
/// past [`HOLD_OFF_MS`] so it cannot be hit by someone meaning to power down.
pub const HOLD_BOOTLOADER_MS: u32 = 8_000;

/// Presses shorter than this are contact bounce, not intent.
pub const DEBOUNCE_MS: u32 = 30;

/// What the button asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    /// Nothing happened this tick.
    None,
    /// A deliberate short press: wake the device / turn the lights back on.
    Wake,
    /// Held past [`HOLD_OFF_MS`]: shut down.
    PowerOff,
    /// Held past [`HOLD_BOOTLOADER_MS`]: reboot into ROM download mode.
    EnterBootloader,
}

/// Debounced press-and-hold recogniser.
#[derive(Debug, Default)]
pub struct PowerButton {
    /// When the current press began, if the button is down.
    down_since: Option<u32>,
    /// Set once a hold has fired, so one long press cannot emit repeatedly.
    fired: bool,
}

impl PowerButton {
    pub fn new() -> PowerButton {
        PowerButton::default()
    }

    /// Feed the debounced button level and the current time.
    ///
    /// Long holds fire **while still held**, which is what makes them feel
    /// right: the device acts at two seconds rather than when you happen to let
    /// go. A short press, by contrast, can only be recognised on release —
    /// until then it is indistinguishable from the start of a hold.
    pub fn update(&mut self, pressed: bool, now_ms: u32) -> PowerAction {
        match (self.down_since, pressed) {
            // Press begins.
            (None, true) => {
                self.down_since = Some(now_ms);
                self.fired = false;
                PowerAction::None
            }
            // Still held: check whether a threshold has just been crossed.
            (Some(start), true) => {
                if self.fired {
                    return PowerAction::None;
                }
                let held = now_ms.wrapping_sub(start);
                if held >= HOLD_BOOTLOADER_MS {
                    self.fired = true;
                    PowerAction::EnterBootloader
                } else if held >= HOLD_OFF_MS {
                    // Not marked as fired: the press may go on to reach the
                    // bootloader threshold, and the caller powering down is
                    // what ends the gesture in the normal case.
                    self.fired = false;
                    PowerAction::PowerOff
                } else {
                    PowerAction::None
                }
            }
            // Released.
            (Some(start), false) => {
                let held = now_ms.wrapping_sub(start);
                self.down_since = None;
                let fired = self.fired;
                self.fired = false;
                if fired || held >= HOLD_OFF_MS {
                    // A hold already did its thing; the release means nothing.
                    PowerAction::None
                } else if held >= DEBOUNCE_MS {
                    PowerAction::Wake
                } else {
                    PowerAction::None // bounce
                }
            }
            (None, false) => PowerAction::None,
        }
    }

    /// True while the button is down.
    pub fn is_down(&self) -> bool {
        self.down_since.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the button through a press held for exactly `hold_ms`, sampling
    /// every 10 ms, and collect everything it emitted. The release lands on
    /// `hold_ms` itself so the hold duration is exactly what the name says.
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
        // The device should act at two seconds, not when you let go.
        let mut b = PowerButton::new();
        b.update(true, 0);
        assert_eq!(b.update(true, HOLD_OFF_MS - 1), PowerAction::None);
        assert_eq!(b.update(true, HOLD_OFF_MS), PowerAction::PowerOff);
    }

    #[test]
    fn a_long_hold_reaches_the_bootloader_and_stops_there() {
        let actions = press_for(HOLD_BOOTLOADER_MS + 200);
        assert!(actions.contains(&PowerAction::EnterBootloader), "{actions:?}");
        // Once the bootloader has fired, holding on must not keep emitting.
        let after: std::vec::Vec<_> = actions
            .iter()
            .skip_while(|a| **a != PowerAction::EnterBootloader)
            .skip(1)
            .collect();
        assert!(after.is_empty(), "kept firing after the bootloader: {after:?}");
    }

    #[test]
    fn the_bootloader_threshold_is_well_clear_of_power_off() {
        // Someone meaning to power down must not land in the bootloader.
        assert!(
            HOLD_BOOTLOADER_MS >= HOLD_OFF_MS * 3,
            "the two gestures are too close together"
        );
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
        // embassy's millisecond counter wraps; a press across the wrap must not
        // read as a multi-hour hold.
        let mut b = PowerButton::new();
        b.update(true, u32::MAX - 50);
        assert_eq!(b.update(true, u32::MAX - 10), PowerAction::None);
        assert_eq!(b.update(false, 10u32.wrapping_sub(0)), PowerAction::Wake);
    }
}
