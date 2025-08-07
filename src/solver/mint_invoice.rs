use std::sync::Arc;

use chrono::{DateTime, Utc};
use eyre::{OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use safe_math::safe;

use crate::solver::solver_order::SolverOrderAssetLot;
use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, PaymentId, Symbol},
    decimal_ext::DecimalExt,
};

use super::index_order::{IndexOrder, IndexOrderUpdate};

pub struct IndexOrderUpdateReport {
    chain_id: u32,
    address: Address,
    symbol: Symbol,
}

impl IndexOrderUpdateReport {
    pub fn new(chain_id: u32, address: Address, symbol: Symbol) -> Self {
        Self {
            chain_id,
            address,
            symbol,
        }
    }

    pub fn report_closed_update(&self, update: Arc<RwLock<IndexOrderUpdate>>) {
        tracing::info_span!("closed-update").in_scope(|| {
            let update_read = update.read();
            tracing::info!(
                "Closing Index Order [{}:{}] {}",
                self.chain_id,
                self.address,
                update_read.client_order_id,
            );
            tracing::info!(
                "{} {} rc={:0.5} cs={:0.5} ec={:0.5} fill={:0.5} fee={:0.5} (closed)",
                update_read.client_order_id,
                self.symbol,
                update_read.remaining_collateral,
                update_read.collateral_spent,
                update_read.engaged_collateral.unwrap_or_default(),
                update_read.filled_quantity,
                update_read.update_fee,
            );
            tracing::info!("");
        });
    }
}

fn print_heading(
    title: &str,
    index_order: &IndexOrder,
    update: &IndexOrderUpdate,
    timestamp: DateTime<Utc>,
) {
    tracing::info_span!("heading").in_scope(|| {
        tracing::info!("== {} == ", title);
        tracing::info!("");
        tracing::info!("Date:  {} ", timestamp);
        tracing::info!("To:    [{}:{}]", index_order.chain_id, index_order.address);
        tracing::info!("Order: {}", update.client_order_id);
        tracing::info!("Index: {}", index_order.symbol);
        tracing::info!("");
        tracing::info!("Collateral Spent: {:0.5}", index_order.collateral_spent);
        tracing::info!("Filed Quantity: {:0.5}", index_order.filled_quantity);
        tracing::info!("");
        tracing::info!("");
    });
}

pub fn print_fill_report(
    index_order: &IndexOrder,
    update: &IndexOrderUpdate,
    fill_amount: Amount,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    tracing::info_span!("fill-report").in_scope(|| {
        print_heading("Fill Report", index_order, update, timestamp);
        tracing::info!("{: ^22}| {: ^10} |{: ^10}", "Item", "Qty", "Amount");
        tracing::info!("{}", (0..46).map(|_| "-").join(""));
        tracing::info!("{: <22}| {: >10.5} |{: ^10}", "Filled", fill_amount, "");
        tracing::info!(
            "{: <22}| {: >10.5} |{: >10.5}",
            "Total Filled",
            update.filled_quantity,
            update.collateral_spent
        );
        tracing::info!("{}", (0..46).map(|_| "-").join(""));
        tracing::info!(
            "{: <22}  {: <10} |{: >10.5}",
            "Collateral",
            "Remaining",
            update.remaining_collateral + update.engaged_collateral.unwrap_or_default()
        );
        tracing::info!("{}", (0..46).map(|_| "-").join(""));
        tracing::info!(
            "{: <22}  {: <10} |{: >10.5}",
            "of which",
            "Engaged",
            update.engaged_collateral.unwrap_or_default()
        );
        tracing::info!(
            "{: <22}  {: <10} |{: >10.5}",
            " ",
            "Unengaged",
            update.remaining_collateral,
        );

        tracing::info!("{}", (0..46).map(|_| "-").join(""));

        tracing::info!("");
        tracing::info!("-- All User Orders --");
        tracing::info!("");
        tracing::info!("To:    [{}:{}]", index_order.chain_id, index_order.address);
        tracing::info!("Index: {}", index_order.symbol);
        tracing::info!("");
        tracing::info!("{}", (0..46).map(|_| "-").join(""));
        tracing::info!(
            "{: <22}| {: >10.5} |{: >10.5}",
            "Total Filled",
            index_order.filled_quantity,
            index_order.collateral_spent
        );
        tracing::info!("{}", (0..46).map(|_| "-").join(""));
        tracing::info!(
            "{: <22}  {: <10} |{: >10.5}",
            "Total Collateral",
            "Remaining",
            index_order.remaining_collateral + index_order.engaged_collateral.unwrap_or_default()
        );

        tracing::info!("");
        Ok(())
    })
}

/// print minting invoice into log
pub fn print_mint_invoice(
    index_order: &IndexOrder,
    update: &IndexOrderUpdate,
    payment_id: &PaymentId,
    amount_paid: Amount,
    lots: Vec<SolverOrderAssetLot>,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    tracing::info_span!("mint-invoice").in_scope(|| {
        print_heading("Mint Invoice", index_order, update, timestamp);
        tracing::info!(
            "{: ^12}| {: ^10} |{: ^10} |{: ^10} |{: ^10} |{: ^10}",
            "Lot",
            "Symbol",
            "Qty",
            "Price",
            "Fee",
            "Amount"
        );

        tracing::info!("{}", (0..72).map(|_| "-").join(""));
        let lots = lots
            .into_iter()
            .sorted_by_cached_key(|x| x.lot_id.cloned())
            .coalesce(|a, b| {
                if a.lot_id.eq(&b.lot_id) {
                    Ok(SolverOrderAssetLot {
                        fee: a.fee + b.fee,
                        quantity: a.quantity + b.quantity,
                        ..a
                    })
                } else {
                    Err((a, b))
                }
            })
            .collect_vec();
        let total_amount: Amount = lots.iter().map(|x| x.quantity * x.price + x.fee).sum();
        let sub_totals = lots
            .iter()
            .map(|x| {
                (
                    x.symbol.clone(),
                    x.quantity,
                    x.fee,
                    x.price * x.quantity + x.fee,
                )
            })
            .sorted_by_cached_key(|(symbol, _, _, _)| symbol.clone())
            .coalesce(|a, b| {
                if a.0.eq(&b.0) {
                    Ok((a.0, a.1 + b.1, a.2 + b.2, a.3 + b.3))
                } else {
                    Err((a, b))
                }
            })
            .collect_vec();
        for lot in lots {
            tracing::info!(
                "{: <12}| {: <10} |{: >10.5} |{: >10.5} |{: >10.5} |{: >10.5}",
                format!("{}", lot.lot_id),
                lot.symbol,
                lot.quantity,
                lot.price,
                lot.fee,
                lot.quantity * lot.price + lot.fee
            )
        }

        tracing::info!("{}", (0..72).map(|_| "-").join(""));
        for sub_total in sub_totals {
            let average_price = (sub_total.3 - sub_total.2) / sub_total.1;
            tracing::info!(
                "{: <12}| {: <10} |{: >10.5} |~{: >9.4} |{: >10.5} |{: >10.5}",
                "Sub-Total",
                sub_total.0,
                sub_total.1,
                average_price,
                sub_total.2,
                sub_total.3
            )
        }

        tracing::info!("{}", (0..72).map(|_| "-").join(""));
        tracing::info!("{: <46} Sub Total     |{: >10.5}", " ", total_amount,);
        tracing::info!(
            "{: <46} Paid          |{: >10.5}",
            format!("{}", payment_id),
            amount_paid,
        );
        tracing::info!(
            "{: <46} Balance       |{: >10.5}",
            " ",
            (total_amount - amount_paid),
        );

        tracing::info!("{}", (0..72).map(|_| "-").join(""));
        let total_collateral =
            total_amount + update.update_fee + index_order.engaged_collateral.unwrap_or_default();
        let total_paid = amount_paid + update.update_fee;
        tracing::info!(
            "{: <46} Deposited     |{: >10.5}",
            "Collateral",
            total_collateral,
        );
        tracing::info!(
            "{: <46} Paid          |{: >10.5}",
            "Management Fee",
            update.update_fee,
        );
        tracing::info!("{: <46} Total Paid    |{: >10.5}", " ", total_paid,);
        tracing::info!(
            "{: <46} Balance       |{: >10.5}",
            "Reached minting threshold before full spend",
            index_order.engaged_collateral.unwrap_or_default()
        );

        tracing::info!("{}", (0..72).map(|_| "-").join(""));
        tracing::info!(
            "{: <46} Fill Rate     |{: >9.5}%",
            "Collateral used",
            Amount::ONE_HUNDRED * total_paid / total_collateral
        );
        tracing::info!(
            "{: <46} Fill Rate     |{: >9.5}%",
            "Less management fee",
            Amount::ONE_HUNDRED * amount_paid / (total_collateral - update.update_fee)
        );
        tracing::info!("");
        Ok(())
    })
}

#[derive(Debug)]
pub struct MintInvoice {
    pub client_order_id: ClientOrderId,
    pub payment_id: PaymentId,
    pub symbol: Symbol,
    pub total_amount: Amount,
    pub amount_paid: Amount,
    pub amount_remaining: Amount,
    pub management_fee: Amount,
    pub assets_value: Amount,
    pub exchange_fee: Amount,
    pub fill_rate: Amount,
    pub lots: Vec<SolverOrderAssetLot>,
    pub timestamp: DateTime<Utc>,
}

impl MintInvoice {
    pub fn try_new(
        index_order: &IndexOrder,
        update: &IndexOrderUpdate,
        payment_id: &PaymentId,
        amount_paid: Amount,
        lots: Vec<SolverOrderAssetLot>,
        timestamp: DateTime<Utc>,
    ) -> Result<Self> {
        print_mint_invoice(
            index_order,
            update,
            payment_id,
            amount_paid,
            lots.clone(),
            timestamp,
        )?;

        let total_amount = update.original_collateral_amount;
        let management_fee = update.update_fee;

        let amount_remaining =
            safe!(safe!(update.original_collateral_amount - amount_paid) - management_fee)
                .ok_or_eyre("Math problem")?;

        let fill_rate = safe!(
            safe!(update.collateral_spent - update.update_fee)
                / safe!(update.original_collateral_amount - update.update_fee)
                    .ok_or_eyre("Math problem")?
        )
        .ok_or_eyre("Math problem")?;

        let assets_value = lots
            .iter()
            .map(|lot| safe!(lot.price * lot.quantity))
            .fold_options(Some(Amount::ZERO), |a, v| safe!(a + v))
            .flatten()
            .ok_or_eyre("Math error")?;

        let exchange_fee = lots
            .iter()
            .map(|lot| Some(lot.fee))
            .fold_options(Some(Amount::ZERO), |a, v| safe!(a + v))
            .flatten()
            .ok_or_eyre("Math error")?;

        Ok(Self {
            client_order_id: update.client_order_id.clone(),
            payment_id: payment_id.clone(),
            symbol: index_order.symbol.clone(),
            total_amount,
            amount_paid,
            amount_remaining,
            management_fee,
            assets_value,
            exchange_fee,
            fill_rate,
            lots,
            timestamp,
        })
    }
}
