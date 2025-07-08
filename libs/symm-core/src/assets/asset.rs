use serde::{Deserialize, Serialize};

use crate::{core::bits::Symbol, string_id};

string_id!(ListingId);

impl Serialize for ListingId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ListingId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(ListingId(s))
    }
}

#[derive(Serialize, Deserialize)]
pub struct Asset {
    pub ticker: Symbol, // add things like:
                        // precision: u8 - number of decimal places
                        // ...(when required)
    pub listing: ListingId,
}

impl Asset {
    pub fn new(name: Symbol, listing: ListingId) -> Self {
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
        assert_eq!(asset_btc.listing.as_str(), "BINANCE");
        Ok(())
    }
}
