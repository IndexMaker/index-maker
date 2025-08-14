use std::collections::{hash_map::Entry, HashMap};

use chrono::{DateTime, Utc};
use eyre::eyre;
use itertools::{chain, Itertools};
use serde::{Deserialize, Serialize};
use serde_json::json;
use symm_core::core::bits::{Amount, Symbol};

use crate::{
    csv_writer::CsvWriter,
    stats_buffer::{Stats, StatsBuffer},
};

#[derive(Serialize, Deserialize)]
pub struct MetricsBufferSide {
    buffers: HashMap<Symbol, Vec<StatsBuffer>>,
}

impl MetricsBufferSide {
    pub fn new() -> Self {
        Self {
            buffers: HashMap::new(),
        }
    }

    /// Push new data layer (dim=2) to the history of data layers (dim=3)
    ///
    /// For each symbol we've got width of values, and here we are appending
    /// depth of values history. StatsBuffer stores history of single parameter,
    /// and we have width of parameters, so we need width of histories. We're
    /// essentially adding a dimension to our tensor, which is basically:
    ///     width x height x depth
    /// where:
    ///     width - represents different bands: 10, 20, 30, ... 100, 150,..., 500bps
    ///     height - represents symbols of traded markets: BTCUSD, ETHUSD, XRPUSD, ...
    ///     depth - represents the history of the parameter value
    ///
    pub fn push(&mut self, map: HashMap<Symbol, Vec<Option<Amount>>>) {
        for (symbol, values) in map {
            match self.buffers.entry(symbol) {
                Entry::Occupied(mut occupied_entry) => {
                    occupied_entry
                        .get_mut()
                        .iter_mut()
                        .zip(values)
                        .for_each(|(b, v)| b.push(v));
                }
                Entry::Vacant(vacant_entry) => {
                    vacant_entry.insert(
                        values
                            .into_iter()
                            .map(|v| {
                                let mut b = StatsBuffer::new();
                                b.push(v);
                                b
                            })
                            .collect_vec(),
                    );
                }
            }
        }
    }

    pub fn flush_stats(
        &mut self,
        labels: &Vec<String>,
    ) -> (
        // Symbol => Label => Stat Header => Value
        HashMap<Symbol, HashMap<String, HashMap<String, String>>>,
        // Flattened array of all errors if any, mainly for logging
        Vec<eyre::Report>,
    ) {
        let mut all_stats = HashMap::new();
        let mut all_errors = Vec::new();

        for (symbol, stats_buffers) in self.buffers.iter_mut() {
            let (stats, errors): (Vec<_>, Vec<_>) = stats_buffers
                .iter_mut()
                .zip(labels.iter())
                .map(|(stats_buffer, label)| match stats_buffer.compute_stats() {
                    Ok(computed_stats) => Ok((label.clone(), stats_buffer, computed_stats)),
                    Err(err) => Err(err),
                })
                .partition_result();

            let stats: HashMap<_, _> = stats
                .into_iter()
                .map(|(label, stats_buffer, computed_stats)| {
                    if let Some(computed_stats) = computed_stats {
                        let values = computed_stats.get_values();
                        (label.to_owned(), values)
                    } else {
                        (
                            label.to_owned(),
                            Stats::get_values_for_all_missing(stats_buffer.get_missing()),
                        )
                    }
                })
                .collect();

            all_stats.insert(symbol.to_owned(), stats);
            all_errors.extend(errors);
        }

        (all_stats, all_errors)
    }
}

pub struct MetricsBuffer {
    created_time: DateTime<Utc>,
    aggregation_period: chrono::Duration,
    labels: Vec<String>,
    bid_buffers: MetricsBufferSide,
    ask_buffers: MetricsBufferSide,
}

impl MetricsBuffer {
    pub fn new(labels: Vec<String>, aggregation_period: chrono::Duration) -> Self {
        Self {
            created_time: Utc::now(),
            aggregation_period,
            labels,
            bid_buffers: MetricsBufferSide::new(),
            ask_buffers: MetricsBufferSide::new(),
        }
    }

    pub fn ingest(
        &mut self,
        bid_bands: HashMap<Symbol, Vec<Option<Amount>>>,
        ask_bands: HashMap<Symbol, Vec<Option<Amount>>>,
    ) -> eyre::Result<bool> {
        tracing::trace!(
            bid_bands = %json!(bid_bands),
            ask_bands = %json!(ask_bands),
            "Bands");

        self.bid_buffers.push(bid_bands);
        self.ask_buffers.push(ask_bands);

        let time = Utc::now();
        let period = time - self.created_time;

        Ok(self.aggregation_period < period)
    }

    pub fn flush_stats(
        &mut self,
        base_asset_by_symbol: &HashMap<Symbol, Symbol>,
        path: &str,
    ) -> eyre::Result<()> {
        tracing::debug!(
            bids = %json!(self.bid_buffers),
            asks = %json!(self.ask_buffers),
            "Flush"
        );

        let created_time = self.created_time;
        let updated_time = Utc::now();

        let prefix_headers: Vec<String> =
            vec!["TimestampStart", "TimestampEnd", "Symbol", "Base Asset"]
                .into_iter()
                .map_into()
                .collect_vec();

        let stats_headers = Stats::get_headers();
        let stats_headers = self
            .labels
            .iter()
            .flat_map(|label| {
                stats_headers
                    .iter()
                    .map(move |header| format!("({}) {}", label, header))
            })
            .collect_vec();

        let bid_stats_headers = stats_headers
            .iter()
            .map(|header| format!("Bid {}", header))
            .collect_vec();

        let ask_stats_headers = stats_headers
            .iter()
            .map(|header| format!("Ask {}", header))
            .collect_vec();

        let headers = chain!(prefix_headers, bid_stats_headers, ask_stats_headers).collect_vec();

        let mut writer = CsvWriter::new(headers);

        let (bid_stats, bid_stats_errors) = self.bid_buffers.flush_stats(&self.labels);
        let (ask_stats, ask_stats_errors) = self.ask_buffers.flush_stats(&self.labels);

        if !bid_stats_errors.is_empty() || !ask_stats_errors.is_empty() {
            tracing::warn!(
                "Some stats failed to compute: {}",
                chain!(bid_stats_errors, ask_stats_errors)
                    .into_iter()
                    .map(|err| format!("{:?}", err))
                    .join(";")
            );
        }

        for (symbol, base_asset) in base_asset_by_symbol {
            let prefix_data = [
                ("TimestampStart", created_time.timestamp().to_string()),
                ("TimestampEnd", updated_time.timestamp().to_string()),
                ("Symbol", symbol.to_string()),
                ("Base Asset", base_asset.to_string()),
            ];

            let mut row_data: HashMap<_, _> = prefix_data
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect();

            let bid_stats = bid_stats.get(symbol);
            let ask_stats = ask_stats.get(symbol);

            for (side, stats) in [("Bid", bid_stats), ("Ask", ask_stats)] {
                if let Some(stats) = stats {
                    row_data.extend(
                        stats
                            .iter()
                            .flat_map(|(label, stats)| {
                                stats.into_iter().map(move |(header, value)| {
                                    (format!("{} ({}) {}", side, label, header), value.clone())
                                })
                            })
                            .collect::<HashMap<_, _>>(),
                    );
                } else {
                    tracing::warn!(
                        "Missing stats for {} {} at {} .. {}",
                        side,
                        symbol,
                        created_time,
                        updated_time
                    );
                }
            }

            writer.push(row_data);
        }

        tracing::info!(
            num_records = %writer.len(),
            %created_time,
            %updated_time,
            %path,
            "Writing records to file");

        writer.write_into_file(path)
    }
}
