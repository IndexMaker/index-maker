use std::collections::HashSet;

use clap::Parser;
use serde::{Deserialize, Serialize};
use symm_core::{
    core::{
        json_file_async::{read_from_json_file_async, write_json_to_file_async},
        logging::log_init,
    },
    init_log,
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(long, short)]
    invalid_symbols: String,

    #[arg(long, short)]
    file: String,

    #[arg(long, short)]
    output_file: String,
}

#[derive(Deserialize)]
struct InvalidSymbols {
    pub invalid_symbols: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct AssetWeight {
    pub id: i32,
    pub pair: String,
    pub listing: String,
    pub assetname: String,
    pub sector: String,
    pub market_cap: i128,
    pub weights: String,
    pub quantity: f64,
}

#[tokio::main]
pub async fn main() {
    init_log!();

    let cli = Cli::parse();

    let invalid_symbols: InvalidSymbols = read_from_json_file_async(&cli.invalid_symbols)
        .await
        .expect("Failed to read invalid symbols");

    tracing::info!(
        "Loaded {} invalid symbols",
        invalid_symbols.invalid_symbols.len()
    );

    let mut asset_weights: Vec<AssetWeight> = read_from_json_file_async(&cli.file)
        .await
        .expect("Failed to read asset weights");

    tracing::info!("Loaded {} asset weights", asset_weights.len());

    let invalid_symbols: HashSet<String> = invalid_symbols.invalid_symbols.into_iter().collect();

    asset_weights.retain(|aw| {
        if invalid_symbols.contains(&aw.pair) {
            tracing::debug!("Removing {:?}", aw);
            false
        } else {
            true
        }
    });

    write_json_to_file_async(&cli.output_file, &asset_weights)
        .await
        .expect("Failed to write output");
}
