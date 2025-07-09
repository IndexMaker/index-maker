use serde::{Deserialize, Serialize};

use crate::core::bits::Symbol;

#[derive(Serialize, Deserialize)]
pub struct Asset {
    #[serde(alias="pair")]
    pub ticker: Symbol, // add things like:
                        // precision: u8 - number of decimal places
                        // ...(when required)

    #[serde(default)]
    pub listing: Symbol,
}

impl Asset {
    pub fn new(name: Symbol, listing: Symbol) -> Self {
        Self { ticker: name, listing }
    }
}

#[cfg(test)]
mod tests {
    use eyre::Result;

    use crate::assets::asset::Asset;

    #[test]
    fn test_asset() -> Result<()> {
        let asset_btc = Asset::new("BTC".into(), "BINANCE".into());
        assert_eq!(asset_btc.ticker.as_ref(), "BTC");
        assert_eq!(asset_btc.listing.as_ref(), "BINANCE");
        Ok(())
    }
}
