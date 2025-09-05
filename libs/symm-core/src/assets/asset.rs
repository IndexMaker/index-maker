use serde::{Deserialize, Serialize};

use crate::core::bits::Symbol;

#[derive(Serialize, Deserialize)]
pub struct Asset {
    #[serde(rename = "pair")]
    pub ticker: Symbol, // add things like:
    // precision: u8 - number of decimal places
    // ...(when required)
    #[serde(default)]
    pub listing: Symbol,
}

impl Asset {
    pub fn new(name: Symbol, listing: Symbol) -> Self {
        Self {
            ticker: name,
            listing,
        }
    }
}

// W/a: We need Asset Manager
pub fn get_base_asset_symbol_workaround(symbol: &Symbol) -> Symbol {
    const KNOWN_SUFFIXES: [&str; 3] = ["USDC", "USDT", "EUR"];

    if let Some(stripped) = KNOWN_SUFFIXES.iter().find_map(|s| symbol.strip_suffix(s)) {
        Symbol::from(stripped)
    } else {
        symbol.clone()
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
