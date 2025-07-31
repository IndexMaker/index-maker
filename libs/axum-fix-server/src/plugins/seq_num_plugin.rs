use crate::messages::{ServerRequest, ServerResponse, SessionId};
use eyre::{OptionExt, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::marker::PhantomData;

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
        self.last_sent_seq_num
            .read()
            .get(&session_id)
            .map_or(0, |v| *v)
    }

    pub fn last_received_seq_num(&self, session_id: &SessionId) -> u32 {
        self.last_received_seq_num
            .read()
            .get(&session_id)
            .map_or(0, |v| *v)
    }

    pub fn valid_seq_num(&self, seq_num: u32, session_id: &SessionId) -> bool {
        let is_valid = self
            .last_received_seq_num
            .read()
            .get(session_id)
            .map_or(0, |v| *v)
            + 1
            == seq_num;

        if is_valid {
            self.last_received_seq_num
                .write()
                .insert(session_id.clone(), seq_num);
            true
        } else {
            false
        }
    }

    pub fn next_seq_num(&self, session_id: &SessionId) -> u32 {
        let mut last_send_seq_num_write = self.last_sent_seq_num.write();
        if let Some(value) = last_send_seq_num_write.get_mut(&session_id) {
            *value += 1;
            *value
        } else {
            last_send_seq_num_write.insert(session_id.clone(), 1);
            1
        }
    }

    pub fn create_session(&self, session_id: &SessionId) -> Result<()> {
        self.last_received_seq_num
            .write()
            .insert(session_id.clone(), 0);

        self.last_sent_seq_num.write().insert(session_id.clone(), 0);

        Ok(())
    }

    pub fn destroy_session(&self, session_id: &SessionId) -> Result<()> {
        self.last_received_seq_num.write().remove(session_id);
        self.last_sent_seq_num.write().remove(session_id);
        Ok(())
    }
}
