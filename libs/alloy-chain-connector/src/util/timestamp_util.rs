use alloy_primitives::U256;
use chrono::{DateTime, Utc};


pub fn timestamp_from_date(date: DateTime<Utc>) -> U256 {
    let _ = date;
    U256::ZERO
}

pub fn date_from_timestamp(timestamp: U256) -> DateTime<Utc> {
    let _ = timestamp;
    DateTime::default()
}