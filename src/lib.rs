pub mod app {
    pub mod application;
    pub mod basket_manager;
    pub mod batch_manager;
    pub mod chain_connector;
    pub mod collateral_manager;
    pub mod collateral_router;
    pub mod config;
    pub mod config_loader;
    pub mod dispatcher;
    pub mod fix_server;
    pub mod index_order_manager;
    pub mod market_data;
    pub mod order_sender;

    pub mod quote_request_manager;
    pub mod simple_chain;
    pub mod simple_router;
    pub mod simple_sender;
    pub mod simple_server;
    pub mod simple_solver;
    pub mod solver;
    pub mod timestamp_ids;
}

pub mod collateral {
    pub mod collateral_manager;
    pub mod collateral_position;
}

pub mod server {
    pub mod server;
    pub mod fix {
        pub mod messages;
        pub mod rate_limit_config;
        pub mod requests;
        pub mod responses;
        pub mod server;
        pub mod server_plugin;
    }
}

pub mod solver {
    pub mod batch_manager;
    pub mod index_order;
    pub mod index_order_manager;
    pub mod index_quote;
    pub mod index_quote_manager;
    pub mod mint_invoice;
    pub mod solver;
    pub mod solver_order;
    pub mod solver_quote;
    pub mod solvers {
        pub mod simple_solver;
    }
}

// Re-export main types from main.rs for external use
pub use crate::app::config::ApplicationConfig;
pub use crate::app::config_loader::ConfigLoader;
pub use crate::app::application::{Application, ApplicationBuilder};

// Re-export CLI types and SecretsProvider from main.rs
// Note: These are defined in main.rs and need to be accessible from lib.rs
// We'll create a separate module for shared types
pub mod shared {
    pub use crate::app::config::{ApplicationConfig, ConfigBuildError};
}

// CLI and command definitions
pub mod cli {
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
}

// Re-export CLI types
pub use cli::{Cli, Commands};

// Secrets provider implementation
use std::collections::HashMap;

pub struct SecretsProvider {
    secrets: HashMap<String, String>,
}

impl SecretsProvider {
    pub fn new(secrets: HashMap<String, String>) -> Self {
        Self { secrets }
    }

    pub fn get_secret(&self, key: &str) -> Option<String> {
        self.secrets.get(key).cloned()
    }

    pub fn has_binance_credentials(&self) -> bool {
        self.secrets.contains_key("binance_api_key") && self.secrets.contains_key("binance_api_secret")
    }

    pub fn has_bitget_credentials(&self) -> bool {
        self.secrets.contains_key("bitget_api_key") && self.secrets.contains_key("bitget_api_secret")
    }

    pub fn has_chain_credentials(&self) -> bool {
        self.secrets.contains_key("chain_private_key")
    }
}
