use clap::{Parser, Subcommand};
use symm_core::core::bits::{Amount, Side, Symbol};

/// Command line interface definition
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Application command to execute
    #[command(subcommand)]
    pub command: Commands,

    /// Main quote currency override
    #[arg(long, short)]
    pub main_quote_currency: Option<Symbol>,

    /// Enable sender simulation mode
    #[arg(long, short)]
    pub simulate_sender: bool,

    /// Enable chain simulation mode
    #[arg(long, short)]
    pub simulate_chain: bool,

    /// Server bind address override
    #[arg(long, short)]
    pub bind_address: Option<String>,

    /// Log file path override
    #[arg(long, short)]
    pub log_path: Option<String>,

    /// Disable terminal logging
    #[arg(long, short)]
    pub term_log_off: bool,

    /// OpenTelemetry trace URL override
    #[arg(long)]
    pub otlp_trace_url: Option<String>,

    /// OpenTelemetry log URL override
    #[arg(long)]
    pub otlp_log_url: Option<String>,

    /// Batch size override
    #[arg(long, short)]
    pub batch_size: Option<usize>,

    /// Configuration file path
    #[arg(long, short)]
    pub config_path: Option<String>,
}

/// Available application commands
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Send a single order
    SendOrder {
        /// Symbol to trade
        symbol: Symbol,
        /// Order side (Buy/Sell)
        side: Side,
        /// Order quantity
        quantity: Amount,
    },
    /// Start FIX server mode
    FixServer {
        /// Collateral amount
        collateral_amount: Amount,
    },
    /// Start quote server mode
    QuoteServer {},
}
