use rust_decimal::Decimal;

pub trait DecimalExt {
    fn checked_math<F>(&self, func: F) -> Option<Decimal>
    where
        F: FnOnce(&Decimal) -> Option<Decimal>;
}

impl DecimalExt for Decimal {
    fn checked_math<F>(&self, func: F) -> Option<Decimal>
    where
        F: FnOnce(&Decimal) -> Option<Decimal>,
    {
        func(self)
    }
}

impl DecimalExt for Option<Decimal> {
    fn checked_math<F>(&self, func: F) -> Option<Decimal>
    where
        F: FnOnce(&Decimal) -> Option<Decimal>,
    {
        self.and_then(|x| func(&x))
    }
}
