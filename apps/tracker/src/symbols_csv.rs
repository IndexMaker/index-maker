use csv::ReaderBuilder;
use itertools::Itertools;
use symm_core::core::bits::Symbol;
use eyre::eyre;

#[derive(Debug, Clone)]
pub struct Asset {
    pub traded_market: Symbol,
    pub base_asset: Symbol,
    pub quote_asset: Symbol,
}

pub struct Assets {
    pub assets: Vec<Asset>,
}

impl Assets {
    pub async fn try_new_from_csv(path: &str) -> eyre::Result<Self> {
        let mut rdr = ReaderBuilder::new()
            .has_headers(true)
            .trim(csv::Trim::All)
            .from_path(path)?;

        let (assets, errors): (Vec<_>, Vec<_>) = rdr
            .records()
            .into_iter()
            .filter_map_ok(|rec| {
                if rec.len() == 3 {
                    Some(Asset {
                        traded_market: Symbol::from(rec[0].trim()),
                        base_asset: Symbol::from(rec[1].trim()),
                        quote_asset: Symbol::from(rec[2].trim()),
                    })
                } else {
                    None
                }
            })
            .partition_result();

        if !errors.is_empty() {
            Err(eyre!(
                "Errors while parsing CSV file: {}",
                errors
                    .into_iter()
                    .map(|err| format!("{:?}", err))
                    .join("; ")
            ))?;
        }

        Ok(Self { assets })
    }
}
