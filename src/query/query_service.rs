use std::sync::Arc;

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
    query::query_service_state::QueryServiceState,
    solver::
        mint_invoice_manager::{GetInvoiceData, GetInvoicesData}
    ,
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

            let api_v1 = Router::new()
                .route(
                    "/mint_invoice/:chain_id/:address/:client_order_id",
                    get(get_mint_invoice),
                )
                .route(
                    "/mint_invoices_in_date_range/:from_date/:to_date",
                    get(get_mint_invoices_in_date_range),
                );

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
/// `/api/v1/mint_invoice/1/0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266/VVY-QNF-EPF-1160`
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
/// `/api/v1/mint_invoices_in_date_range/2025-03-05T00:00:00.000Z/2025-03-05T23:00:00.000Z`
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
