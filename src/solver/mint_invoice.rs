use std::sync::Arc;

use chrono::{DateTime, Utc};
use eyre::{OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;

use crate::{core::bits::{Address, Amount, ClientOrderId, PaymentId, Symbol}, solver::index_order};

use super::{
    index_order::{IndexOrder, IndexOrderUpdate},
    solver::SolverOrderAssetLot,
};

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
        println!(
            "(index-order-manager) Closing Index Order [{}:{}] {} {} rc={:0.5} cs={:0.5} fill={:0.5} fee={:0.5}",
            self.chain_id,
            self.address,
            self.symbol,
            update_read.client_order_id,
            update_read.remaining_collateral,
            update_read.collateral_spent,
            update_read.filled_quantity,
            update_read.update_fee,
        );
    }
}

/// print minting invoice into log
pub fn print_mint_invoice(
    index_order: &Arc<RwLock<IndexOrder>>,
    update: &Arc<RwLock<IndexOrderUpdate>>,
    payment_id: &PaymentId,
    amount_paid: Amount,
    lots: Vec<SolverOrderAssetLot>,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    let index_order_read = index_order.read();
    let update_read = update.read();
    println!("(index-order-manager) == Mint Invoice == ");
    println!("(index-order-manager)");
    println!("(index-order-manager) Date:  {} ", timestamp);
    println!("(index-order-manager) To:    [{}:{}]", index_order_read.chain_id, index_order_read.address);
    println!("(index-order-manager) Order: {}", update_read.client_order_id);
    println!("(index-order-manager) Index: {}", index_order_read.symbol);
    println!("(index-order-manager)");
    println!(
        "(index-order-manager) {: ^12}| {: ^10} |{: ^10} |{: ^10} |{: ^10} |{: ^10}",
        "Lot", "Symbol", "Qty", "Price", "Fee", "Amount"
    );
    println!("(index-order-manager) {}", (0..72).map(|_| "-").join(""));
    let lots = lots
        .into_iter()
        .sorted_by_cached_key(|x| x.lot_id.0.clone())
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
        .sorted_by_cached_key(|x| x.0.clone())
        .coalesce(|a, b| {
            if a.0.eq(&b.0) {
                Ok((a.0, a.1 + b.1, a.2 + b.2, a.3 + b.3))
            } else {
                Err((a, b))
            }
        })
        .collect_vec();
    for lot in lots {
        println!(
            "(index-order-manager) {: <12}| {: <10} |{: >10.5} |{: >10.5} |{: >10.5} |{: >10.5}",
            format!("{}", lot.lot_id),
            lot.symbol,
            lot.quantity,
            lot.price,
            lot.fee,
            lot.quantity * lot.price + lot.fee
        )
    }
    println!("(index-order-manager) {}", (0..72).map(|_| "-").join(""));
    for sub_total in sub_totals {
        let average_price = (sub_total.3 - sub_total.2) / sub_total.1;
        println!(
            "(index-order-manager) {: <12}| {: <10} |{: >10.5} |~{: >9.4} |{: >10.5} |{: >10.5}",
            "Sub-Total", sub_total.0, sub_total.1, average_price, sub_total.2, sub_total.3
        )
    }
    println!("(index-order-manager) {}", (0..72).map(|_| "-").join(""));
    println!(
        "(index-order-manager) {: <46} Sub Total     |{: >10.5}",
        " ", total_amount,
    );
    println!(
        "(index-order-manager) {: <46} Paid          |{: >10.5}",
        format!("{}", payment_id),
        amount_paid,
    );
    println!(
        "(index-order-manager) {: <46} Balance       |{: >10.5}",
        " ",
        (total_amount - amount_paid),
    );
    println!("(index-order-manager) {}", (0..72).map(|_| "-").join(""));
    println!(
        "(index-order-manager) {: <46} Deposited     |{: >10.5}",
        "Collateral",
        total_amount + update_read.update_fee + index_order_read.engaged_collateral.unwrap_or_default()
    );
    println!(
        "(index-order-manager) {: <46} Paid          |{: >10.5}",
        "Management Fee",
        update_read.update_fee,
    );
    println!(
        "(index-order-manager) {: <46} Total Paid    |{: >10.5}",
        " ", total_amount + update_read.update_fee
    );
    println!(
        "(index-order-manager) {: <46} Balance       |{: >10.5}",
        "Reached minting threshold before full spend",
        index_order_read.engaged_collateral.unwrap_or_default()
    );
    println!("(index-order-manager)");
    Ok(())
}
