use eyre::{OptionExt, Result};
use rust_decimal::Decimal;

pub trait DecimalExt {
    fn checked_math<F>(&self, func: F) -> Result<Decimal>
    where
        F: FnOnce(&Decimal) -> Option<Decimal>;
}

impl DecimalExt for Decimal {
    fn checked_math<F>(&self, func: F) -> Result<Decimal>
    where
        F: FnOnce(&Decimal) -> Option<Decimal>,
    {
        Ok(func(self).ok_or_eyre("Math overflow")?)
    }
}

pub trait OptionDecimalExt {
    fn update_with<F>(&mut self, func: F) -> Result<Decimal>
    where
        F: FnOnce(Option<&Decimal>) -> Option<Decimal>;
}

impl OptionDecimalExt for Option<Decimal> {
    fn update_with<F>(&mut self, func: F) -> Result<Decimal>
    where
        F: FnOnce(Option<&Decimal>) -> Option<Decimal>,
    {
        let value = func(self.as_ref()).ok_or_eyre("Math overflow")?;
        self.replace(value);
        Ok(value)
    }
}

