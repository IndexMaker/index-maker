use std::sync::Arc;

use chrono::{DateTime, Utc};
use eyre::{OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use safe_math::safe;

use crate::{
    core::{
        bits::{Address, Amount, ClientOrderId, PaymentId, Symbol},
        decimal_ext::DecimalExt,
    },
    solver::index_order,
};

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
            "(report) Closing Index Order [{}:{}] {}",
            self.chain_id, self.address, update_read.client_order_id,
        );
        println!(
            "(report) {} {} rc={:0.5} cs={:0.5} ec={:0.5} fill={:0.5} fee={:0.5} (closed)",
            update_read.client_order_id,
            self.symbol,
            update_read.remaining_collateral,
            update_read.collateral_spent,
            update_read.engaged_collateral.unwrap_or_default(),
            update_read.filled_quantity,
            update_read.update_fee,
        );
        println!("(report)");
    }
}

fn print_heading(
    title: &str,
    index_order_read: &IndexOrder,
    update_read: &IndexOrderUpdate,
    timestamp: DateTime<Utc>,
) {
    println!("(report) == {} == ", title);
    println!("(report)");
    println!("(report) Date:  {} ", timestamp);
    println!(
        "(report) To:    [{}:{}]",
        index_order_read.chain_id, index_order_read.address
    );
    println!("(report) Order: {}", update_read.client_order_id);
    println!("(report) Index: {}", index_order_read.symbol);
    println!("(report)");
}

pub fn print_fill_report(
    index_order_read: &IndexOrder,
    update_read: &IndexOrderUpdate,
    fill_amount: Amount,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    print_heading("Fill Report", index_order_read, update_read, timestamp);
    println!(
        "(report) {: ^22}| {: ^10} |{: ^10}",
        "Item", "Qty", "Amount"
    );
    println!("(report) {}", (0..46).map(|_| "-").join(""));
    println!(
        "(report) {: <22}| {: >10.5} |{: ^10}",
        "Filled", fill_amount, ""
    );
    println!(
        "(report) {: <22}| {: >10.5} |{: >10.5}",
        "Total Filled", update_read.filled_quantity, update_read.collateral_spent
    );
    println!("(report) {}", (0..46).map(|_| "-").join(""));
    println!(
        "(report) {: <22}  {: <10} |{: >10.5}",
        "Collateral",
        "Engaged",
        update_read.engaged_collateral.unwrap_or_default()
    );
    println!(
        "(report) {: <22}  {: <10} |{: >10.5}",
        " ", "Remaining", update_read.remaining_collateral,
    );

    println!("(report) {}", (0..46).map(|_| "-").join(""));

    println!("(report)");
    println!("(report) -- All User Orders --");
    println!("(report)");
    println!(
        "(report) To:    [{}:{}]",
        index_order_read.chain_id, index_order_read.address
    );
    println!("(report) Index: {}", index_order_read.symbol);
    println!("(report)");
    println!("(report) {}", (0..46).map(|_| "-").join(""));
    println!(
        "(report) {: <22}| {: >10.5} |{: >10.5}",
        "Total Filled", index_order_read.filled_quantity, index_order_read.collateral_spent
    );
    println!("(report) {}", (0..46).map(|_| "-").join(""));
    println!(
        "(report) {: <22}  {: <10} |{: >10.5}",
        "Collateral",
        "Engaged",
        index_order_read.engaged_collateral.unwrap_or_default()
    );
    println!(
        "(report) {: <22}  {: <10} |{: >10.5}",
        " ", "Remaining", index_order_read.remaining_collateral,
    );

    println!("(report)");
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
    println!(
        "(report) {: ^12}| {: ^10} |{: ^10} |{: ^10} |{: ^10} |{: ^10}",
        "Lot", "Symbol", "Qty", "Price", "Fee", "Amount"
    );
    println!("(report) {}", (0..72).map(|_| "-").join(""));
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
            "(report) {: <12}| {: <10} |{: >10.5} |{: >10.5} |{: >10.5} |{: >10.5}",
            format!("{}", lot.lot_id),
            lot.symbol,
            lot.quantity,
            lot.price,
            lot.fee,
            lot.quantity * lot.price + lot.fee
        )
    }
    println!("(report) {}", (0..72).map(|_| "-").join(""));
    for sub_total in sub_totals {
        let average_price = (sub_total.3 - sub_total.2) / sub_total.1;
        println!(
            "(report) {: <12}| {: <10} |{: >10.5} |~{: >9.4} |{: >10.5} |{: >10.5}",
            "Sub-Total", sub_total.0, sub_total.1, average_price, sub_total.2, sub_total.3
        )
    }
    println!("(report) {}", (0..72).map(|_| "-").join(""));
    println!(
        "(report) {: <46} Sub Total     |{: >10.5}",
        " ", total_amount,
    );
    println!(
        "(report) {: <46} Paid          |{: >10.5}",
        format!("{}", payment_id),
        amount_paid,
    );
    println!(
        "(report) {: <46} Balance       |{: >10.5}",
        " ",
        (total_amount - amount_paid),
    );
    println!("(report) {}", (0..72).map(|_| "-").join(""));
    let total_collateral = total_amount
        + update_read.update_fee
        + index_order_read.engaged_collateral.unwrap_or_default();
    let total_paid = amount_paid + update_read.update_fee;
    println!(
        "(report) {: <46} Deposited     |{: >10.5}",
        "Collateral", total_collateral,
    );
    println!(
        "(report) {: <46} Paid          |{: >10.5}",
        "Management Fee", update_read.update_fee,
    );
    println!("(report) {: <46} Total Paid    |{: >10.5}", " ", total_paid,);
    println!(
        "(report) {: <46} Balance       |{: >10.5}",
        "Reached minting threshold before full spend",
        index_order_read.engaged_collateral.unwrap_or_default()
    );
    println!("(report) {}", (0..72).map(|_| "-").join(""));
    println!(
        "(report) {: <46} Fill Rate     |{: >9.5}%",
        "Collateral used",
        Amount::ONE_HUNDRED * total_paid / total_collateral
    );
    println!(
        "(report) {: <46} Fill Rate     |{: >9.5}%",
        "Less management fee",
        Amount::ONE_HUNDRED * amount_paid / (total_collateral - update_read.update_fee)
    );
    println!("(report)");
    Ok(())
}
