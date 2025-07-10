use crate::messages::{ServerRequest, ServerResponse, SessionId};
use eyre::{Report, Result};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::RwLock;

pub trait WithSeqNumPlugin {
    fn get_seq_num(&self) -> u32;
    fn set_seq_num(&mut self, seq_num: u32);
}

pub struct SeqNumPlugin<R, Q> {
    last_received_seq_num: RwLock<HashMap<SessionId, u32>>,
    last_sent_seq_num: RwLock<HashMap<SessionId, u32>>,
    _phantom_r: PhantomData<R>,
    _phantom_q: PhantomData<Q>,
}

impl<R, Q> SeqNumPlugin<R, Q>
where
    R: ServerRequest + WithSeqNumPlugin,
    Q: ServerResponse + WithSeqNumPlugin,
{
    pub fn new() -> Self {
        //TODO: load a seqnum cache from memory on creation, and save on destruction
        Self {
            _phantom_r: PhantomData::default(),
            _phantom_q: PhantomData::default(),
            last_received_seq_num: RwLock::new(HashMap::new()),
            last_sent_seq_num: RwLock::new(HashMap::new()),
        }
    }

    pub fn last_sent_seq_num(&self, session_id: &SessionId) -> u32 {
        let read_lock = self.last_sent_seq_num.read().unwrap();
        read_lock.get(&session_id).map_or(0, |v| *v)
    }

    pub fn last_received_seq_num(&self, session_id: &SessionId) -> u32 {
        let read_lock = self.last_received_seq_num.read().unwrap();
        read_lock.get(&session_id).map_or(0, |v| *v)
    }

    pub fn valid_seq_num(&self, seq_num: u32, session_id: &SessionId) -> bool {
        let read_lock = self.last_received_seq_num.read().unwrap();
        let is_valid = read_lock.get(session_id).map_or(0, |v| *v) + 1 == seq_num;
        drop(read_lock); // Release the read lock before acquiring write lock

        if is_valid {
            // Acquire a write lock to update the value
            let mut write_lock = self.last_received_seq_num.write().unwrap();
            write_lock.insert(session_id.clone(), seq_num);
            true
        } else {
            false
        }
    }

    pub fn next_seq_num(&self, session_id: &SessionId) -> u32 {
        let mut write_lock = self.last_sent_seq_num.write().unwrap();
        if let Some(value) = write_lock.get_mut(&session_id) {
            *value += 1;
            *value
        } else {
            write_lock.insert(session_id.clone(), 1);
            1
        }
    }

    pub fn create_session(&self, session_id: &SessionId) -> Result<(), Report> {
        let mut write_lock = self.last_received_seq_num.write().unwrap();
        write_lock.insert(session_id.clone(), 0);
        drop(write_lock);

        let mut write_lock = self.last_sent_seq_num.write().unwrap();
        write_lock.insert(session_id.clone(), 0);
        drop(write_lock);

        Ok(())
    }

    pub fn destroy_session(&self, session_id: &SessionId) -> Result<(), Report> {
        let mut write_lock = self.last_received_seq_num.write().unwrap();
        write_lock.remove(&session_id.clone());
        drop(write_lock);

        let mut write_lock = self.last_sent_seq_num.write().unwrap();
        write_lock.remove(&session_id.clone());
        drop(write_lock);

        Ok(())
    }
}
