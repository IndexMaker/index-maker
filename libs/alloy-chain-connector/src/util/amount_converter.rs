use alloy_primitives::{
    utils::{format_units, parse_units},
    U256,
};
use symm_core::core::bits::Amount;


#[derive(Clone)]
pub struct AmountConverter {
    decimals: u8,
}

impl AmountConverter {
    pub fn new(decimals: u8) -> Self {
        Self { decimals }
    }

    pub fn into_amount(&self, value: U256) -> eyre::Result<Amount> {
        let formatted = format_units(value, self.decimals).map_err(|err| {
            eyre::eyre!(
                "Failed to format U256 to units with {} decimals: {}",
                self.decimals,
                err
            )
        })?;

        let amount = formatted
            .parse()
            .map_err(|err| eyre::eyre!("Failed to convert formatted string to Amount: {}", err))?;

        Ok(amount)
    }

    pub fn from_amount(&self, amount: Amount) -> eyre::Result<U256> {
        let amount_string = amount.to_string();

        let parsed_units = parse_units(&amount_string, self.decimals).map_err(|err| {
            eyre::eyre!(
                "Failed to parse amount '{}' with {} decimals: {}",
                amount_string,
                self.decimals,
                err
            )
        })?;

        Ok(parsed_units.into())
    }
}
