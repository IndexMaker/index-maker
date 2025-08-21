use crate::util::amount_converter::AmountConverter;
use alloy_primitives::U256;
use alloy_rpc_types_eth::TransactionReceipt;
use symm_core::core::bits::Amount;


pub fn compute_gas_used(
    converter: &AmountConverter,
    receipt: TransactionReceipt,
) -> eyre::Result<Amount> {
    let gas_price = U256::try_from(receipt.effective_gas_price)?;
    let gas_used = U256::try_from(receipt.gas_used)?;

    let gas_amount = converter.into_amount(gas_price * gas_used)?;
    Ok(gas_amount)
}
