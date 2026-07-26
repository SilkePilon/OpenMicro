//! The physical key layout, and what each key is *for*.
//!
//! This is shared vocabulary rather than firmware-private detail: the daemon
//! needs it to decide which keys to light and to route a press onto an action,
//! and the firmware needs it to decide which LED to paint. Putting it in one
//! place is what stops the two ends disagreeing about which key is "Deny".
//!
//! # Where these numbers come from
//!
//! Not guessed. Work Louder's shipping firmware embeds its own default
//! `keymap.json`, and one of the profiles in it is an **agent** profile — the
//! vendor's own take on exactly this use case:
//!
//! ```text
//! "keymap": [
//!   ["KV_OAI_AG00",  "KV_OAI_AG01"],
//!   ["KV_OAI_AG02",  "KV_OAI_AG03",  "KV_OAI_AG04",  "KV_OAI_AG05"],
//!   ["KV_OAI_ACT06", "KV_OAI_ACT07", "KV_OAI_ACT08", "KV_OAI_ACT09"],
//!   ["KV_OAI_ACT10", "KV_OAI_ACT11", "KV_OAI_ACT12"]
//! ]
//! ```
//!
//! So the board is 13 keys in rows of **2, 4, 4, 3**, numbered row-major from
//! the top left, and the vendor splits them into six `AG` (agent) keys and
//! seven `ACT` (action) keys. That six is why [`crate::SLOT_COUNT`] is six.
//!
//! # The physical picture
//!
//! ```text
//!         ┌────┐┌────┐
//!         │  0 ││  1 │            row 0 — agent slots 0..1
//!         └────┘└────┘
//!   ┌────┐┌────┐┌────┐┌────┐
//!   │  2 ││  3 ││  4 ││  5 │      row 1 — agent slots 2..5
//!   └────┘└────┘└────┘└────┘
//!   ┌────┐┌────┐┌────┐┌────┐
//!   │  6 ││  7 ││  8 ││  9 │      row 2 — reserved, dark
//!   └────┘└────┘└────┘└────┘
//!   ┌────┐┌────┐┌────┐
//!   │ 10 ││ 11 ││ 12 │            row 3 — Approve / Always / Deny
//!   └────┘└────┘└────┘
//! ```

/// Mechanical switches on the board.
pub const KEY_COUNT: usize = 13;

/// Keys per row, top to bottom. Sums to [`KEY_COUNT`].
pub const ROWS: [u8; 4] = [2, 4, 4, 3];

/// Addressable underglow LEDs, in a ring.
pub const UNDERGLOW_COUNT: usize = 8;

/// Keys that show an agent slot, indexed by slot number.
pub const AGENT_KEYS: [u8; 6] = [0, 1, 2, 3, 4, 5];

/// Stop whatever the selected session is doing.
///
/// Row 2's leftmost key. It lives on its own row rather than in the bottom
/// three because it answers a different question: the bottom row responds to a
/// request the agent made, while this one interrupts work nobody asked about.
/// Mixing it into the decision row would make "deny" and "stop" neighbours,
/// which is one wrong press away from being annoying.
pub const INTERRUPT_KEY: u8 = 6;

/// Keys with no assigned meaning yet. Held dark on purpose: an unlit key reads
/// as "nothing here", where a lit one invites a press that does nothing.
pub const RESERVED_KEYS: [u8; 3] = [7, 8, 9];

/// Allow the pending action once.
pub const APPROVE_KEY: u8 = 10;
/// Allow it, and stop asking for this kind of action.
pub const ALWAYS_KEY: u8 = 11;
/// Reject the pending action.
pub const DENY_KEY: u8 = 12;

/// The bottom row, left to right.
pub const ACTION_KEYS: [u8; 3] = [APPROVE_KEY, ALWAYS_KEY, DENY_KEY];

/// Chain position of the LED under a given key.
///
/// **This is the one number here that is not confirmed.** Everything else came
/// out of the vendor's keymap; the order the WS2812 chain is physically routed
/// in did not, because nothing in their image states it — the strip is written
/// as a flat array and the wiring is a PCB fact.
///
/// Identity is the reasonable default: a board whose keys are numbered
/// row-major is normally wired row-major too, and the vendor's own agent
/// profile assumes key index and slot index line up. If the lights land on the
/// wrong keys, this table is the only thing that needs changing — which is
/// exactly why it exists instead of the identity being spread across the
/// render code.
///
/// To confirm it: light one LED at a time and note which key it appears under
/// (the firmware has an identify mode for this).
pub const LED_FOR_KEY: [u8; KEY_COUNT] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

/// What a key means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRole {
    /// Shows, and focuses, agent slot `n`.
    Agent(u8),
    /// Stops the selected session.
    Interrupt,
    /// No meaning yet.
    Reserved,
    Approve,
    Always,
    Deny,
}

/// One of the three bottom-row decision keys.
///
/// Split out from [`KeyRole`] so code that only ever deals with the action row
/// does not have to handle `Agent`/`Reserved` cases that cannot occur there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRole {
    Approve,
    Always,
    Deny,
}

impl ActionRole {
    /// The key id this role sits on.
    pub const fn key(self) -> u8 {
        match self {
            Self::Approve => APPROVE_KEY,
            Self::Always => ALWAYS_KEY,
            Self::Deny => DENY_KEY,
        }
    }

    /// Its backlight colour.
    pub const fn color(self) -> crate::Rgb {
        match self {
            Self::Approve => crate::APPROVE_COLOR,
            Self::Always => crate::ALWAYS_COLOR,
            Self::Deny => crate::DENY_COLOR,
        }
    }

    /// Bottom row, left to right.
    pub const ALL: [ActionRole; 3] = [Self::Approve, Self::Always, Self::Deny];
}

/// The role of a key id, or `None` if it is off the board.
///
/// Total over `0..KEY_COUNT` by construction — the exhaustiveness test below
/// keeps it that way if the rows are ever renumbered.
pub const fn role_of(key: u8) -> Option<KeyRole> {
    match key {
        0..=5 => Some(KeyRole::Agent(key)),
        INTERRUPT_KEY => Some(KeyRole::Interrupt),
        7..=9 => Some(KeyRole::Reserved),
        APPROVE_KEY => Some(KeyRole::Approve),
        ALWAYS_KEY => Some(KeyRole::Always),
        DENY_KEY => Some(KeyRole::Deny),
        _ => None,
    }
}

/// The (row, column) a key sits at, for laying out a preview in the TUI.
pub const fn row_col(key: u8) -> Option<(u8, u8)> {
    let mut row = 0;
    let mut first = 0u8;
    while row < ROWS.len() {
        let width = ROWS[row];
        if key < first + width {
            return Some((row as u8, key - first));
        }
        first += width;
        row += 1;
    }
    None
}

/// Rows sum to the key count, and the LED table covers every key exactly once.
const _: () = {
    let mut total = 0;
    let mut i = 0;
    while i < ROWS.len() {
        total += ROWS[i] as usize;
        i += 1;
    }
    assert!(total == KEY_COUNT, "ROWS must account for every key");

    // A duplicated or out-of-range LED index would leave one key permanently
    // dark and another double-driven, which is maddening to debug by eye.
    let mut i = 0;
    while i < KEY_COUNT {
        assert!((LED_FOR_KEY[i] as usize) < KEY_COUNT, "LED index off the chain");
        let mut j = i + 1;
        while j < KEY_COUNT {
            assert!(LED_FOR_KEY[i] != LED_FOR_KEY[j], "two keys share one LED");
            j += 1;
        }
        i += 1;
    }

    assert!(AGENT_KEYS.len() == crate::SLOT_COUNT, "one agent key per slot");
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_exactly_one_role() {
        for key in 0..KEY_COUNT as u8 {
            assert!(role_of(key).is_some(), "key {key} has no role");
        }
        assert_eq!(role_of(KEY_COUNT as u8), None, "off-board keys have no role");
        assert_eq!(role_of(255), None);
    }

    #[test]
    fn agent_keys_map_to_their_slot_number() {
        for (slot, key) in AGENT_KEYS.iter().enumerate() {
            assert_eq!(role_of(*key), Some(KeyRole::Agent(slot as u8)));
        }
    }

    #[test]
    fn the_bottom_row_is_the_three_action_keys() {
        // The user-facing promise: bottom row, left to right, is
        // approve / always / deny. If a renumbering breaks that, the labels on
        // a physical device silently become wrong.
        let bottom_row_start = KEY_COUNT as u8 - ROWS[3];
        for (i, key) in (bottom_row_start..KEY_COUNT as u8).enumerate() {
            assert_eq!(key, ACTION_KEYS[i], "bottom row position {i}");
        }
        assert_eq!(role_of(ACTION_KEYS[0]), Some(KeyRole::Approve));
        assert_eq!(role_of(ACTION_KEYS[1]), Some(KeyRole::Always));
        assert_eq!(role_of(ACTION_KEYS[2]), Some(KeyRole::Deny));
    }

    #[test]
    fn action_keys_are_not_agent_keys() {
        for k in ACTION_KEYS {
            assert!(!AGENT_KEYS.contains(&k), "key {k} is both an agent and an action key");
        }
    }

    #[test]
    fn row_col_walks_the_rows_in_order() {
        assert_eq!(row_col(0), Some((0, 0)));
        assert_eq!(row_col(1), Some((0, 1)));
        assert_eq!(row_col(2), Some((1, 0)));
        assert_eq!(row_col(5), Some((1, 3)));
        assert_eq!(row_col(6), Some((2, 0)));
        assert_eq!(row_col(9), Some((2, 3)));
        assert_eq!(row_col(10), Some((3, 0)));
        assert_eq!(row_col(12), Some((3, 2)));
        assert_eq!(row_col(13), None);
    }

    #[test]
    fn row_col_is_consistent_with_the_row_widths() {
        for key in 0..KEY_COUNT as u8 {
            let (row, col) = row_col(key).expect("every key sits somewhere");
            assert!(col < ROWS[row as usize], "key {key} is off the end of row {row}");
        }
    }
}
