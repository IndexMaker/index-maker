use crate::messages::SessionId;
use eyre::Result;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use symm_core::core::bits::Address;

pub trait WithUserPlugin {
    fn get_user_id(&self) -> (u32, Address);
}

pub struct UserPlugin {
    users_sessions: RwLock<HashMap<(u32, Address), HashSet<SessionId>>>,
}

impl UserPlugin {
    pub fn new() -> Self {
        Self {
            users_sessions: RwLock::new(HashMap::new()),
        }
    }

    pub fn add_user_session(&self, user_id: &(u32, Address), session_id: &SessionId) {
        let mut write_lock = self.users_sessions.write().unwrap();
        write_lock
            .entry(*user_id)
            .or_insert(HashSet::new())
            .insert(session_id.clone());
    }

    pub fn remove_user_session(&self, user_id: &(u32, Address), session_id: &SessionId) {
        let mut write_lock = self.users_sessions.write().unwrap();
        if let Some(sessions) = write_lock.get_mut(user_id) {
            sessions.remove(session_id);
            if sessions.is_empty() {
                write_lock.remove(user_id);
            }
        }
    }

    pub fn remove_session(&self, session_id: &SessionId) {
        let user_id_option = {
            let read_lock = self.users_sessions.read().unwrap();
            read_lock.iter().find_map(|(key, values)| {
                if values.contains(session_id) {
                    Some(key.clone())
                } else {
                    None
                }
            })
        };

        if let Some(user_id) = user_id_option {
            self.remove_user_session(&user_id, session_id);
        }
    }

    pub fn get_user_sessions(&self, user_id: &(u32, Address)) -> Result<HashSet<SessionId>> {
        let read_lock = self.users_sessions.read().unwrap();

        if let Some(sessions) = read_lock.get(user_id) {
            Ok(sessions.clone())
        } else {
            Err(eyre::eyre!(
                "User not found: ({}, {})",
                user_id.0,
                user_id.1
            ))
        }
    }
}
