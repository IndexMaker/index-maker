
#[cfg(test)]
use eyre::Result;

use crate::core::bits::Symbol;

pub struct Asset {
    pub name: Symbol
    // add things like:
    // precision: u8 - number of decimal places
    // ...(when required)
}

impl Asset {
    pub fn new(name: Symbol) -> Self {
        Self {
            name
        }
    }
}

#[test]
fn test_asset() -> Result<()> {
    let asset_btc = Asset::new("BTC".into());
    assert_eq!(asset_btc.name.as_ref(), "BTC");
    Ok(())
}