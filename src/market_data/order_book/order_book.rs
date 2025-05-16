use std::sync::Arc;

use crossbeam::atomic::AtomicCell;
use intrusive_collections::{
    intrusive_adapter,
    rbtree::{Cursor, CursorMut},
    Bound, KeyAdapter, RBTree, RBTreeAtomicLink,
};
use rust_decimal::Decimal;
use safe_math::safe;

use crate::core::{
    bits::{Amount, PricePointEntry, Side},
    decimal_ext::DecimalExt,
};

use eyre::{eyre, Ok, Result};

pub struct PricePointBookEntry {
    price: Amount,
    quantity: AtomicCell<Decimal>,
    link: RBTreeAtomicLink,
}

intrusive_adapter!(pub PricePointBookEntryAdapter = Arc<PricePointBookEntry>: PricePointBookEntry { link: RBTreeAtomicLink });

impl<'a> KeyAdapter<'a> for PricePointBookEntryAdapter {
    type Key = Amount;
    fn get_key(&self, value: &'a PricePointBookEntry) -> Self::Key {
        value.price
    }
}

/// implement price level order book allowing to inspect market depth
pub struct PricePointEntries {
    side: Side,
    tolerance: Amount,
    entries: RBTree<PricePointBookEntryAdapter>,
}

struct PricePointEntriesOps {
    side: Side,
    bound: Amount,
}

impl PricePointEntriesOps {
    fn try_new(side: Side, tolerance: Amount, price: Amount) -> Option<Self> {
        let bound = match side {
            Side::Buy => safe!(price - tolerance)?,
            Side::Sell => safe!(price + tolerance)?,
        };
        Some(Self { side, bound })
    }

    fn begin_ops<'a>(
        &self,
        entries: &'a RBTree<PricePointBookEntryAdapter>,
    ) -> Cursor<'a, PricePointBookEntryAdapter> {
        match self.side {
            Side::Buy => entries.back(),
            Side::Sell => entries.front(),
        }
    }

    fn move_next<'a>(&self, cursor: &mut Cursor<'a, PricePointBookEntryAdapter>) {
        match self.side {
            Side::Buy => cursor.move_prev(),
            Side::Sell => cursor.move_next(),
        }
    }

    fn is_finished(&self, price: Amount) -> bool {
        match self.side {
            Side::Buy => price < self.bound,
            Side::Sell => price > self.bound,
        }
    }
}

impl PricePointEntries {
    pub fn new(side: Side, tolerance: Amount) -> Self {
        Self {
            side,
            tolerance,
            entries: RBTree::default(),
        }
    }

    /// Find entry that matches the price with tolerance
    fn find_entry(
        &mut self,
        price: &Amount,
    ) -> Result<(bool, CursorMut<'_, PricePointBookEntryAdapter>)> {
        let price_lower = safe!(*price - self.tolerance).ok_or(eyre!("Math overflow"))?;

        let price_upper = safe!(*price + self.tolerance).ok_or(eyre!("Math overflow"))?;

        let cursor = self.entries.lower_bound_mut(Bound::Included(&price_lower));

        if let Some(entry) = cursor.get() {
            if entry.price < price_upper {
                Ok((true, cursor))
            } else {
                Ok((false, cursor))
            }
        } else {
            Ok((false, cursor))
        }
    }

    fn insert_or_modify_entry(&mut self, entry: &PricePointEntry) -> Result<()> {
        match self.find_entry(&entry.price)? {
            (true, cursor) => {
                if let Some(cursor_entry) = cursor.get() {
                    cursor_entry.quantity.store(entry.quantity);
                }
            }
            (false, mut cursor) => {
                cursor.insert_before(Arc::new(PricePointBookEntry {
                    price: entry.price,
                    quantity: AtomicCell::new(entry.quantity),
                    link: RBTreeAtomicLink::new(),
                }));
            }
        }
        Ok(())
    }

    fn remove_entry(&mut self, entry: &PricePointEntry) -> Result<()> {
        if let (true, mut cursor) = self.find_entry(&entry.price)? {
            cursor.remove();
        }
        Ok(())
    }

    pub fn update(&mut self, entry: &PricePointEntry) -> Result<()> {
        if entry.quantity > self.tolerance {
            self.insert_or_modify_entry(entry)
        } else {
            self.remove_entry(entry)
        }
    }

    pub fn get_liquidity(&self, price: &Amount) -> Result<Amount> {
        let ops = PricePointEntriesOps::try_new(self.side, self.tolerance, price.clone())
            .ok_or(eyre!("Math overflow"))?;

        let mut cursor = ops.begin_ops(&self.entries);
        let mut liquidity = Amount::ZERO;

        while let Some(entry) = cursor.get() {
            if ops.is_finished(entry.price) {
                break;
            }
            liquidity = safe!(liquidity + entry.quantity.load()).ok_or(eyre!("Math overflow"))?;
            ops.move_next(&mut cursor);
        }

        Ok(liquidity)
    }
}

pub struct PricePointBook {
    bid_entries: PricePointEntries,
    ask_entries: PricePointEntries,
}

impl PricePointBook {
    pub fn new(tolerance: Amount) -> Self {
        Self {
            bid_entries: PricePointEntries::new(Side::Buy, tolerance),
            ask_entries: PricePointEntries::new(Side::Sell, tolerance),
        }
    }

    pub fn update_entries(
        &mut self,
        bid_updates: &Vec<PricePointEntry>,
        ask_updates: &Vec<PricePointEntry>,
    ) -> Result<()> {
        for entry in bid_updates {
            self.bid_entries.update(entry)?
        }
        for entry in ask_updates {
            self.ask_entries.update(entry)?
        }
        Ok(())
    }

    pub fn get_liquidity(&self, side: Side, price: &Amount) -> Result<Amount> {
        match side {
            Side::Buy => self.bid_entries.get_liquidity(price),
            Side::Sell => self.ask_entries.get_liquidity(price),
        }
    }
}

#[cfg(test)]
pub mod test {
    use rust_decimal::dec;

    use crate::{
        assert_decimal_approx_eq,
        core::bits::{Amount, PricePointEntry, Side},
    };

    use super::PricePointBook;

    #[test]
    fn test_price_point_order_book() {
        let tolerance = dec!(0.001);

        let mut book = PricePointBook::new(tolerance);

        // Test that empty book has zero liquidity
        let liquidity = book.get_liquidity(Side::Sell, &dec!(110.0));

        assert!(matches!(liquidity, Ok(_)));
        assert_decimal_approx_eq!(liquidity.unwrap(), Amount::ZERO, tolerance);

        // Test that book with single Sell side, has liquidity on the Sell side
        let update_result = book.update_entries(
            &vec![],
            &vec![
                PricePointEntry {
                    price: dec!(100.0),
                    quantity: dec!(10.0),
                },
                PricePointEntry {
                    price: dec!(110.0),
                    quantity: dec!(20.0),
                },
                PricePointEntry {
                    price: dec!(120.0),
                    quantity: dec!(30.0),
                },
            ],
        );

        assert!(matches!(update_result, Ok(_)));

        let liquidity_result = book.get_liquidity(Side::Sell, &dec!(110.0));

        assert!(matches!(liquidity_result, Ok(_)));

        assert_decimal_approx_eq!(liquidity_result.unwrap(), dec!(30.0), tolerance);

        // Test that book with Buy and Sell side, has liquidity on the Buy side
        let update_result = book.update_entries(
            &vec![
                PricePointEntry {
                    price: dec!(90.0),
                    quantity: dec!(50.0),
                },
                PricePointEntry {
                    price: dec!(80.0),
                    quantity: dec!(60.0),
                },
                PricePointEntry {
                    price: dec!(70.0),
                    quantity: dec!(70.0),
                },
            ],
            &vec![],
        );

        assert!(matches!(update_result, Ok(_)));

        let liquidity_result = book.get_liquidity(Side::Buy, &dec!(80.0));

        assert!(matches!(liquidity_result, Ok(_)));

        assert_decimal_approx_eq!(liquidity_result.unwrap(), dec!(110.0), tolerance);

        // Test that price point entry can be removed and updated
        let update_result = book.update_entries(
            &vec![],
            &vec![
                PricePointEntry {
                    price: dec!(100.0),
                    quantity: dec!(0.0),
                },
                PricePointEntry {
                    price: dec!(110.0),
                    quantity: dec!(25.0),
                },
            ],
        );

        assert!(matches!(update_result, Ok(_)));

        let liquidity_result = book.get_liquidity(Side::Sell, &dec!(110.0));

        assert!(matches!(liquidity_result, Ok(_)));

        assert_decimal_approx_eq!(liquidity_result.unwrap(), dec!(25.0), tolerance);
    }
}
