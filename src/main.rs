use clap::{Parser, Subcommand};
use eyre::Result;
use symm_core::core::bits::{Amount, Side, Symbol};

use index_maker::app::{
    application::ApplicationBuilder,
    config_loader::ApplicationConfigLoader,
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(long, short)]
    pub main_quote_currency: Option<Symbol>,

    #[arg(long, short)]
    pub simulate_sender: bool,

    #[arg(long)]
    pub simulate_chain: bool,

    #[arg(long, short)]
    pub bind_address: Option<String>,

    #[arg(long, short)]
    pub log_path: Option<String>,

    #[arg(long, short)]
    pub config_path: Option<String>,

    #[arg(long, short, action = clap::ArgAction::SetTrue)]
    pub term_log_off: bool,

    #[arg(long)]
    pub otlp_trace_url: Option<String>,

    #[arg(long)]
    pub otlp_log_url: Option<String>,

    #[arg(long)]
    pub batch_size: Option<usize>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    SendOrder {
        side: Side,
        symbol: Symbol,
        collateral_amount: Amount,
    },
    FixServer {
        collateral_amount: Amount,
    },
    QuoteServer {},
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Load configuration from multiple sources with proper precedence:
    // 1. Default values (lowest priority)
    // 2. Configuration file
    // 3. Environment variables
    // 4. Command line arguments (highest priority)
    let config = ApplicationConfigLoader::auto_load(&cli)
        .map_err(|e| eyre::eyre!("Failed to load configuration: {}", e))?;

    // Log the command being executed
    match &cli.command {
        Commands::SendOrder {
            side,
            symbol,
            collateral_amount,
        } => tracing::info!("Index Order: {} {:?} {}", symbol, side, collateral_amount),
        Commands::FixServer { collateral_amount } => {
            tracing::info!("FIX Server: {}", collateral_amount)
        }
        Commands::QuoteServer {} => tracing::info!("Quote FIX Server"),
    }

    // Build the application using the configuration
    let mut app = ApplicationBuilder::new(config)
        .build()
        .await
        .map_err(|e| eyre::eyre!("Failed to build application: {}", e))?;

    // Run the application
    tracing::info!("Starting index-maker application...");
    
    match app.run().await {
        Ok(()) => {
            tracing::info!("Application stopped gracefully.");
            Ok(())
        }
        Err(e) => {
            tracing::error!("Application error: {}", e);
            Err(e)
        }
    }
}

// Re-export the CLI types for use in other modules
pub use Commands;
pub use Cli;
