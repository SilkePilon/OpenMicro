use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use openmicro_proto::{AgentKind, AgentState};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub agent: String,
    pub session: String,
}

impl SessionKey {
    pub fn kind(&self) -> AgentKind {
        AgentKind::from_name(&self.agent)
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub key: SessionKey,
    pub state: AgentState,
    pub updated_ms: u64,
}

impl Session {
    pub fn kind(&self) -> AgentKind {
        self.key.kind()
    }
}

#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: HashMap<SessionKey, Session>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, agent: &str, session: &str, state: AgentState) -> SessionKey {
        let key = SessionKey { agent: agent.to_string(), session: session.to_string() };
        let entry = self.sessions.entry(key.clone()).or_insert_with(|| Session {
            key: key.clone(),
            state,
            updated_ms: 0,
        });
        entry.state = state;
        entry.updated_ms = now_ms();
        key
    }

    #[allow(dead_code)]
    pub fn get(&self, key: &SessionKey) -> Option<&Session> {
        self.sessions.get(key)
    }

    pub fn remove(&mut self, key: &SessionKey) {
        self.sessions.remove(key);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Session> {
        self.sessions.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_inserts_and_mutates() {
        let mut store = SessionStore::new();
        let k = store.update("claude", "abc", AgentState::Thinking);
        assert_eq!(store.get(&k).unwrap().state, AgentState::Thinking);
        store.update("claude", "abc", AgentState::Working);
        assert_eq!(store.get(&k).unwrap().state, AgentState::Working);
        assert_eq!(store.iter().count(), 1);
    }
}
