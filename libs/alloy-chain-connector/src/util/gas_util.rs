use alloy_primitives::{utils::Unit, U256};
use alloy_rpc_types_eth::TransactionReceipt;
use symm_core::core::bits::Amount;

use crate::util::amount_converter::AmountConverter;

pub fn compute_gas_used(receipt: TransactionReceipt) -> eyre::Result<Amount> {
    let gas_price = U256::from(receipt.effective_gas_price);
    let gas_used = U256::from(receipt.gas_used);

    let converter = AmountConverter::new(Unit::ETHER.get());
    let gas_amount = converter.into_amount(gas_price * gas_used)?;

    tracing::info!("⚙️ Converting gas amount: {}(wei) x {}(units) => {}(eth)", gas_price, gas_used, gas_amount);

    Ok(gas_amount)
}
