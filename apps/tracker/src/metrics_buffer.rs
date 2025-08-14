use std::{
    collections::{
        hash_map::Entry,
        HashMap,
    },
    sync::Arc,
};

use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt};
use itertools::Itertools;
use parking_lot::RwLock;
use safe_math::safe;
use serde_json::json;
use symm_core::{
    core::{
        bits::{Amount, PriceType, Side, Symbol},
        decimal_ext::DecimalExt,
    },
    market_data::{order_book::order_book_manager::OrderBookManager, price_tracker::PriceTracker},
};

pub struct MetricsBuffer {
    created_time: DateTime<Utc>,
    aggregation_period: chrono::Duration,
}

impl MetricsBuffer {
    pub fn new(aggregation_period: chrono::Duration) -> Self {
        Self {
            created_time: Utc::now(),
            aggregation_period,
        }
    }

    pub fn ingest(
        &mut self,
        labels: &Vec<String>,
        bid_bands: HashMap<Symbol, Vec<Amount>>,
        ask_bands: HashMap<Symbol, Vec<Amount>>,
    ) -> eyre::Result<bool> {

        // TODO: Replace tracing::info!() with actual buffering of metrics
        // Should use new class for data. Use labels.

        // * labels - 10, 20, 30, ..., 400, 500 - correspond to amounds in hash-maps
        // * bid_bands - maps symbol to vector of values corresponding to labels 
        // * ask_bands - maps symbol to vector of values corresponding to labels 

        // Note: the values are cumulative, i.e. first value would correspond to
        // 0..10, next 0..20, and next 0..30, and so on. To obtain 10..20 values
        // need to be subtracted., e.g. to get 10..20 need to subtract first
        // from second value. 

        _ = labels;
        tracing::info!(
            bid_bands = %json!(bid_bands),
            ask_bands = %json!(ask_bands),
            "Bands");

        let time = Utc::now();
        let period = time - self.created_time;
        let should_flush = self.aggregation_period < period;

        if should_flush {
            // TODO: Compute stats and write to CSV
            // Should be done in new classes
        }

        Ok(should_flush)
    }
}
