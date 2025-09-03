use std::{ops::Deref, sync::Arc};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use std::net::SocketAddr;
use symm_core::core::bits::{Address, ClientOrderId};

use crate::{
    collateral::collateral_position::CollateralPosition,
    query::query_service_state::QueryServiceState,
    solver::mint_invoice_manager::{GetInvoiceData, GetInvoicesData},
};

pub struct QueryService {
    service_state: Arc<QueryServiceState>,
}

impl QueryService {
    pub fn new(state: QueryServiceState) -> Self {
        Self {
            service_state: Arc::new(state),
        }
    }

    pub async fn stop(&self) -> eyre::Result<()> {
        Ok(())
    }

    pub async fn start(&self, address: String) -> eyre::Result<()> {
        let service_state = self.service_state.clone();

        tokio::spawn(async move {
            let addr: SocketAddr = address.parse().expect(&format!(
                "Server failed to start: Invalid address ({})",
                address
            ));
            tracing::info!("Listening on {}", addr);

            let collateral_position_api =
                Router::new().route("/position/:chain_id/:address", get(get_collateral_position));

            let mint_invoice_api = Router::new()
                .route(
                    "/invoice/:chain_id/:address/:client_order_id",
                    get(get_mint_invoice),
                )
                .route(
                    "/from/:from_date/to/:to_date",
                    get(get_mint_invoices_in_date_range),
                );

            let api_v1 = Router::new()
                .nest("/collateral", collateral_position_api)
                .nest("/mint_invoices", mint_invoice_api);

            let app = Router::new()
                .nest("/api/v1", api_v1)
                .with_state(service_state);

            if let Err(e) = axum_server::bind(addr).serve(app.into_make_service()).await {
                tracing::warn!("Server failed to start: {}", e);
            }
        });

        Ok(())
    }
}

/// Example URL:
/// `/api/v1/collateral/position/1/0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266`
async fn get_collateral_position(
    State(state): State<Arc<QueryServiceState>>,
    Path((chain_id, address)): Path<(u32, Address)>,
) -> Result<Json<CollateralPosition>, StatusCode> {
    let manager = state
        .get_collateral_manager()
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if let Some(position) = manager.get_position(chain_id, &address) {
        let position = position.load().deref().deref().clone();
        Ok(Json(position))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Example URL:
/// `/api/v1/mint_invoices/invoice/1/0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266/VVY-QNF-EPF-1160`
async fn get_mint_invoice(
    State(state): State<Arc<QueryServiceState>>,
    Path((chain_id, address, client_order_id)): Path<(u32, Address, ClientOrderId)>,
) -> Result<Json<GetInvoiceData>, StatusCode> {
    if let Some(invoice) = state
        .get_invoice_manager()
        .read()
        .get_invoice(chain_id, address, &client_order_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Ok(Json(invoice))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Example URL:
/// `/api/v1/mint_invoices/from/2025-09-03T00:00:00.000Z/to/2025-09-4T00:00:00.000Z`
async fn get_mint_invoices_in_date_range(
    State(state): State<Arc<QueryServiceState>>,
    Path((from_date, to_date)): Path<(DateTime<Utc>, DateTime<Utc>)>,
) -> Result<Json<Vec<GetInvoicesData>>, StatusCode> {
    let invoices = state
        .get_invoice_manager()
        .read()
        .get_invoices_in_date_range(from_date, to_date)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(invoices))
}
