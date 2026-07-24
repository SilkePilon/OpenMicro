use openmicro_proto::AgentState;

use crate::session::{Session, SessionKey};

/// Choose which session owns the deck. Rules, in order:
/// 1. If any session is AwaitingApproval, the most-recently-updated such one wins (preempt).
/// 2. Else if `pinned` names a live session, it wins.
/// 3. Else the most-recently-updated session wins.
pub fn pick_owner<'a>(
    sessions: impl Iterator<Item = &'a Session>,
    pinned: Option<&SessionKey>,
) -> Option<SessionKey> {
    let all: Vec<&Session> = sessions.collect();

    let mut awaiting: Vec<&Session> = all
        .iter()
        .copied()
        .filter(|s| s.state == AgentState::AwaitingApproval)
        .collect();
    if !awaiting.is_empty() {
        awaiting.sort_by_key(|s| s.updated_ms);
        return awaiting.last().map(|s| s.key.clone());
    }

    if let Some(p) = pinned {
        if all.iter().any(|s| &s.key == p) {
            return Some(p.clone());
        }
    }

    all.iter().max_by_key(|s| s.updated_ms).map(|s| s.key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionStore;

    fn store_with(entries: &[(&str, &str, AgentState)]) -> SessionStore {
        let mut store = SessionStore::new();
        for (a, s, st) in entries {
            store.update(a, s, *st);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        store
    }

    #[test]
    fn most_recent_wins_by_default() {
        let store = store_with(&[
            ("claude", "a", AgentState::Working),
            ("codex", "b", AgentState::Working),
        ]);
        let owner = pick_owner(store.iter(), None).unwrap();
        assert_eq!(owner.agent, "codex");
    }

    #[test]
    fn awaiting_approval_preempts() {
        let store = store_with(&[
            ("claude", "a", AgentState::AwaitingApproval),
            ("codex", "b", AgentState::Working),
        ]);
        let owner = pick_owner(store.iter(), None).unwrap();
        assert_eq!(owner.agent, "claude");
    }
}
