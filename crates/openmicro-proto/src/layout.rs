pub const KEY_COUNT: usize = 13;

pub const ROWS: [u8; 4] = [2, 4, 4, 3];

pub const UNDERGLOW_COUNT: usize = 8;

pub const AGENT_KEYS: [u8; 6] = [0, 1, 2, 3, 4, 5];

pub const INTERRUPT_KEY: u8 = 6;

pub const RESERVED_KEYS: [u8; 3] = [7, 8, 9];

pub const APPROVE_KEY: u8 = 10;
pub const DENY_KEY: u8 = 11;

pub const STATUS_KEY: u8 = 12;

pub const ACTION_KEYS: [u8; 2] = [APPROVE_KEY, DENY_KEY];

pub const LED_FOR_KEY: [u8; KEY_COUNT] = [12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRole {
    Agent(u8),
    Interrupt,
    Reserved,
    Approve,
    Deny,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRole {
    Approve,
    Deny,
}

impl ActionRole {
    pub const fn key(self) -> u8 {
        match self {
            Self::Approve => APPROVE_KEY,
            Self::Deny => DENY_KEY,
        }
    }

    pub const fn color(self) -> crate::Rgb {
        match self {
            Self::Approve => crate::APPROVE_COLOR,
            Self::Deny => crate::DENY_COLOR,
        }
    }

    pub const ALL: [ActionRole; 2] = [Self::Approve, Self::Deny];
}

pub const fn role_of(key: u8) -> Option<KeyRole> {
    match key {
        0..=5 => Some(KeyRole::Agent(key)),
        INTERRUPT_KEY => Some(KeyRole::Interrupt),
        7..=9 => Some(KeyRole::Reserved),
        APPROVE_KEY => Some(KeyRole::Approve),
        DENY_KEY => Some(KeyRole::Deny),
        STATUS_KEY => Some(KeyRole::Status),
        _ => None,
    }
}

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

const _: () = {
    let mut total = 0;
    let mut i = 0;
    while i < ROWS.len() {
        total += ROWS[i] as usize;
        i += 1;
    }
    assert!(total == KEY_COUNT, "ROWS must account for every key");

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
        let bottom_row_start = KEY_COUNT as u8 - ROWS[3];
        assert_eq!(bottom_row_start, APPROVE_KEY, "approve starts the bottom row");
        assert_eq!(role_of(ACTION_KEYS[0]), Some(KeyRole::Approve));
        assert_eq!(role_of(ACTION_KEYS[1]), Some(KeyRole::Deny));
        assert_eq!(role_of(STATUS_KEY), Some(KeyRole::Status));
        assert_eq!(STATUS_KEY, KEY_COUNT as u8 - 1);
        assert_eq!(LED_FOR_KEY[STATUS_KEY as usize], 0);
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
