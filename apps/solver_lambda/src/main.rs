use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use clap::Parser;
use solver_lambda::{
    solver_input::SolverInput, solver_output::SolverOutput, solver_service::SolverService,
};
use std::{net::SocketAddr, sync::Arc};
use symm_core::{core::logging::log_init, init_log};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    bind_address: Option<String>,

    /* TODO: put service configuration parameters */
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    init_log!();

    let service = Arc::new(SolverService::new(/* TODO: use configuration parameters */));

    let address = cli.bind_address.unwrap_or(String::from("127.0.0.1:3099"));

    let addr: SocketAddr = address.parse().expect(&format!(
        "Server failed to start: Invalid address ({})",
        address
    ));
    tracing::info!("Listening on {}", addr);

    let api_v1 = Router::new().route("/solver/tick", post(solver_tick));

    let app = Router::new().nest("/api/v1", api_v1).with_state(service);

    if let Err(e) = axum_server::bind(addr).serve(app.into_make_service()).await {
        tracing::warn!("Server failed to start: {}", e);
    }
}

async fn solver_tick(
    State(service): State<Arc<SolverService>>,
    Json(input): Json<SolverInput>,
) -> Result<Json<SolverOutput>, StatusCode> {
    if let Ok(output) = service.solve(input).await {
        Ok(Json(output))
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    }
}
