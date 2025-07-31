use std::sync::Arc;

use chrono::{DateTime, Utc};
use eyre::Result;
use itertools::Itertools;
use parking_lot::RwLock;
use rust_decimal::Decimal;

use crate::solver::solver_order::SolverOrderAssetLot;
use symm_core::core::bits::{Address, Amount, ClientOrderId, PaymentId, Symbol};

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
        let update_read = update.read();
        tracing::info!(
            "(report) Closing Index Order [{}:{}] {}",
            self.chain_id,
            self.address,
            update_read.client_order_id,
        );
        tracing::info!(
            "(report) {} {} rc={:0.5} cs={:0.5} ec={:0.5} fill={:0.5} fee={:0.5} (closed)",
            update_read.client_order_id,
            self.symbol,
            update_read.remaining_collateral,
            update_read.collateral_spent,
            update_read.engaged_collateral.unwrap_or_default(),
            update_read.filled_quantity,
            update_read.update_fee,
        );
        tracing::info!("(report)");
    }
}

fn print_heading(
    title: &str,
    index_order_read: &IndexOrder,
    update_read: &IndexOrderUpdate,
    timestamp: DateTime<Utc>,
) {
    tracing::info!("(report) == {} == ", title);
    tracing::info!("(report)");
    tracing::info!("(report) Date:  {} ", timestamp);
    tracing::info!(
        "(report) To:    [{}:{}]",
        index_order_read.chain_id,
        index_order_read.address
    );
    tracing::info!("(report) Order: {}", update_read.client_order_id);
    tracing::info!("(report) Index: {}", index_order_read.symbol);
    tracing::info!("(report)");
    tracing::info!(
        "(report) Collateral Spent: {:0.5}",
        index_order_read.collateral_spent
    );
    tracing::info!(
        "(report) Filed Quantity: {:0.5}",
        index_order_read.filled_quantity
    );
    tracing::info!("(report)");
    tracing::info!("(report)");
}

pub fn print_fill_report(
    index_order_read: &IndexOrder,
    update_read: &IndexOrderUpdate,
    fill_amount: Amount,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    print_heading("Fill Report", index_order_read, update_read, timestamp);
    tracing::info!(
        "(report) {: ^22}| {: ^10} |{: ^10}",
        "Item",
        "Qty",
        "Amount"
    );
    tracing::info!("(report) {}", (0..46).map(|_| "-").join(""));
    tracing::info!(
        "(report) {: <22}| {: >10.5} |{: ^10}",
        "Filled",
        fill_amount,
        ""
    );
    tracing::info!(
        "(report) {: <22}| {: >10.5} |{: >10.5}",
        "Total Filled",
        update_read.filled_quantity,
        update_read.collateral_spent
    );
    tracing::info!("(report) {}", (0..46).map(|_| "-").join(""));
    tracing::info!(
        "(report) {: <22}  {: <10} |{: >10.5}",
        "Collateral",
        "Remaining",
        update_read.remaining_collateral + update_read.engaged_collateral.unwrap_or_default()
    );
    tracing::info!("(report) {}", (0..46).map(|_| "-").join(""));
    tracing::info!(
        "(report) {: <22}  {: <10} |{: >10.5}",
        "of which",
        "Engaged",
        update_read.engaged_collateral.unwrap_or_default()
    );
    tracing::info!(
        "(report) {: <22}  {: <10} |{: >10.5}",
        " ",
        "Unengaged",
        update_read.remaining_collateral,
    );

    tracing::info!("(report) {}", (0..46).map(|_| "-").join(""));

    tracing::info!("(report)");
    tracing::info!("(report) -- All User Orders --");
    tracing::info!("(report)");
    tracing::info!(
        "(report) To:    [{}:{}]",
        index_order_read.chain_id,
        index_order_read.address
    );
    tracing::info!("(report) Index: {}", index_order_read.symbol);
    tracing::info!("(report)");
    tracing::info!("(report) {}", (0..46).map(|_| "-").join(""));
    tracing::info!(
        "(report) {: <22}| {: >10.5} |{: >10.5}",
        "Total Filled",
        index_order_read.filled_quantity,
        index_order_read.collateral_spent
    );
    tracing::info!("(report) {}", (0..46).map(|_| "-").join(""));
    tracing::info!(
        "(report) {: <22}  {: <10} |{: >10.5}",
        "Total Collateral",
        "Remaining",
        index_order_read.remaining_collateral
            + index_order_read.engaged_collateral.unwrap_or_default()
    );

    tracing::info!("(report)");
    Ok(())
}

/// print minting invoice into log
pub fn print_mint_invoice(
    index_order_read: &IndexOrder,
    update_read: &IndexOrderUpdate,
    payment_id: &PaymentId,
    amount_paid: Amount,
    lots: Vec<SolverOrderAssetLot>,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    print_heading("Mint Invoice", index_order_read, update_read, timestamp);
    tracing::info!(
        "(report) {: ^12}| {: ^10} |{: ^10} |{: ^10} |{: ^10} |{: ^10}",
        "Lot",
        "Symbol",
        "Qty",
        "Price",
        "Fee",
        "Amount"
    );

    tracing::info!("(report) {}", (0..72).map(|_| "-").join(""));
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
            "(report) {: <12}| {: <10} |{: >10.5} |{: >10.5} |{: >10.5} |{: >10.5}",
            format!("{}", lot.lot_id),
            lot.symbol,
            lot.quantity,
            lot.price,
            lot.fee,
            lot.quantity * lot.price + lot.fee
        )
    }

    tracing::info!("(report) {}", (0..72).map(|_| "-").join(""));
    for sub_total in sub_totals {
        let average_price = (sub_total.3 - sub_total.2) / sub_total.1;
        tracing::info!(
            "(report) {: <12}| {: <10} |{: >10.5} |~{: >9.4} |{: >10.5} |{: >10.5}",
            "Sub-Total",
            sub_total.0,
            sub_total.1,
            average_price,
            sub_total.2,
            sub_total.3
        )
    }

    tracing::info!("(report) {}", (0..72).map(|_| "-").join(""));
    tracing::info!(
        "(report) {: <46} Sub Total     |{: >10.5}",
        " ",
        total_amount,
    );
    tracing::info!(
        "(report) {: <46} Paid          |{: >10.5}",
        format!("{}", payment_id),
        amount_paid,
    );
    tracing::info!(
        "(report) {: <46} Balance       |{: >10.5}",
        " ",
        (total_amount - amount_paid),
    );

    tracing::info!("(report) {}", (0..72).map(|_| "-").join(""));
    let total_collateral = total_amount
        + update_read.update_fee
        + index_order_read.engaged_collateral.unwrap_or_default();
    let total_paid = amount_paid + update_read.update_fee;
    tracing::info!(
        "(report) {: <46} Deposited     |{: >10.5}",
        "Collateral",
        total_collateral,
    );
    tracing::info!(
        "(report) {: <46} Paid          |{: >10.5}",
        "Management Fee",
        update_read.update_fee,
    );
    tracing::info!("(report) {: <46} Total Paid    |{: >10.5}", " ", total_paid,);
    tracing::info!(
        "(report) {: <46} Balance       |{: >10.5}",
        "Reached minting threshold before full spend",
        index_order_read.engaged_collateral.unwrap_or_default()
    );

    tracing::info!("(report) {}", (0..72).map(|_| "-").join(""));
    tracing::info!(
        "(report) {: <46} Fill Rate     |{: >9.5}%",
        "Collateral used",
        Amount::ONE_HUNDRED * total_paid / total_collateral
    );
    tracing::info!(
        "(report) {: <46} Fill Rate     |{: >9.5}%",
        "Less management fee",
        Amount::ONE_HUNDRED * amount_paid / (total_collateral - update_read.update_fee)
    );
    tracing::info!("(report)");
    Ok(())
}

#[derive(Debug)]
pub struct MintInvoice {
    pub timestamp: DateTime<Utc>,
    pub order_id: ClientOrderId,
    pub index_id: Symbol,
    pub collateral_spent: Decimal,
    pub total_collateral: Decimal,
    pub engaged_collateral: Decimal,
    pub management_fee: Decimal,
    pub payment_id: PaymentId,
    pub amount_paid: Amount,
    pub lots: Vec<SolverOrderAssetLot>,
}

impl MintInvoice {
    pub fn try_new(
        index_order_read: &IndexOrder,
        update_read: &IndexOrderUpdate,
        payment_id: &PaymentId,
        amount_paid: Amount,
        lots: Vec<SolverOrderAssetLot>,
        timestamp: DateTime<Utc>,
    ) -> Result<Self> {
        let lots_clone = lots.clone();

        print_mint_invoice(
            index_order_read,
            update_read,
            payment_id,
            amount_paid,
            lots,
            timestamp,
        )?;

        let total_amount: Amount = lots_clone
            .iter()
            .map(|x| x.quantity * x.price + x.fee)
            .sum();
        Ok(Self {
            timestamp,
            order_id: update_read.client_order_id.clone(),
            index_id: index_order_read.symbol.clone(),
            collateral_spent: index_order_read.collateral_spent,
            total_collateral: total_amount
                + update_read.update_fee
                + index_order_read.engaged_collateral.unwrap_or_default(),
            engaged_collateral: index_order_read.engaged_collateral.unwrap_or_default(),
            management_fee: update_read.update_fee,
            payment_id: payment_id.clone(),
            amount_paid,
            lots: lots_clone,
        })
    }
}
