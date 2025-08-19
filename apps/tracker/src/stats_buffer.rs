use std::collections::HashMap;

use eyre::OptionExt;
use rust_decimal::{dec, prelude::ToPrimitive, MathematicalOps};
use safe_math::safe;
use serde::{Deserialize, Serialize};
use symm_core::core::{bits::Amount, decimal_ext::DecimalExt};

fn format_amount(value: Amount) -> String {
    value.round_dp(2).to_string()
}

#[derive(Default, Clone)]
pub struct Stats {
    pub n: usize,
    pub missing: usize,
    pub mean: Amount,
    pub std: Amount,
    pub min: Amount,
    pub p05: Amount,
    pub p25: Amount,
    pub p50: Amount,
    pub p75: Amount,
    pub p95: Amount,
    pub max: Amount,
}

pub const SAMPLES: &str = "Samples";
pub const MISSING: &str = "Missing";

impl Stats {
    pub fn get_headers() -> [&'static str; 11] {
        [
            SAMPLES, MISSING, "Mean", "Std", "Min", "P05", "P25", "P50", "P75", "P95", "Max",
        ]
    }

    pub fn get_values(&self) -> HashMap<String, String> {
        let data = vec![
            self.n.to_string(),
            self.missing.to_string(),
            format_amount(self.mean),
            format_amount(self.std),
            format_amount(self.min),
            format_amount(self.p05),
            format_amount(self.p25),
            format_amount(self.p50),
            format_amount(self.p75),
            format_amount(self.p95),
            format_amount(self.max),
        ];
        let map: HashMap<String, String> = Self::get_headers()
            .into_iter()
            .zip(data.into_iter())
            .map(|(header, value)| (header.to_owned(), value))
            .collect();
        map
    }

    pub fn get_values_for_all_missing(num_samples: usize) -> HashMap<String, String> {
        [
            (SAMPLES.to_owned(), num_samples.to_string()),
            (MISSING.to_owned(), num_samples.to_string()),
        ]
        .into()
    }
}

#[derive(Serialize, Deserialize)]
pub struct StatsBuffer {
    data: Vec<Amount>,
    missing: usize,
    is_sorted: bool,
}

impl StatsBuffer {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            missing: 0usize,
            is_sorted: true,
        }
    }

    pub fn push(&mut self, value: Option<Amount>) {
        if let Some(value) = value {
            self.data.push(value);
            self.is_sorted = false;
        } else {
            self.missing += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn get_missing(&self) -> usize {
        self.missing
    }

    pub fn sort(&mut self) {
        self.data.sort_unstable();
        self.is_sorted = true;
    }

    pub fn nearest_rank(&self, q: Amount) -> eyre::Result<Option<Amount>> {
        if self.is_empty() {
            return Ok(None);
        }

        self.is_sorted
            .then_some(())
            .ok_or_eyre("Data must be sorted first")?;

        let len = self.data.len();
        let index = safe!(q * Amount::from(len))
            .ok_or_eyre("Math problem")?
            .ceil()
            .to_usize()
            .ok_or_eyre("Failed to convert calculated index to integer value")?
            .clamp(1, len)
            - 1;

        Ok(Some(self.data[index]))
    }

    pub fn compute_stats(&mut self) -> eyre::Result<Option<Stats>> {
        if self.is_empty() {
            // Here we bail if there is no data. Otherwise there is data here
            // and below unwraps must not panic, as otherwise something is wrong
            // with the code. We need to distinguish when something is genuine
            // run-time error and then we use Err, and when something is coding
            // error and we use unwrap or expect in that case, e.g. if we add
            // element to a vector, we can expect that this vector is not empty,
            // because otherwise something is wrong with our code adding an
            // element. However if we have some random value, and we perform
            // computation, the result is unknown at compile-time, and thus it
            // would be run-time error if computation failed.  In such case we
            // shall return Err(eyre!("Math problem")), because those types of
            // error are not fault of the code, but fault of the data.
            return Ok(None);
        }

        if !self.is_sorted {
            self.sort();
        }

        let len = self.data.len();

        // Mean, Variance & Standard Deviation
        let sum = self
            .data
            .iter()
            .fold(Some(Amount::ZERO), |a, &v| safe!(a + v))
            .ok_or_eyre("Math problem")?;

        let mean = safe!(sum / Amount::from(len)).ok_or_eyre("Math problem")?;
        let varinace = self
            .data
            .iter()
            .fold(Some(Amount::ZERO), |a, &v| {
                a.checked_math(|&x| safe!(safe!(v * v) + x))
            })
            .checked_math(|&sum_of_squares| safe!(sum_of_squares / Amount::from(len)))
            .checked_math(|&avg_sum_of_squares| safe!(avg_sum_of_squares - safe!(mean * mean)?))
            .ok_or_eyre("Failed to compute variance")?
            .max(Amount::ZERO);

        let std_dev = safe!(varinace.sqrt()).ok_or_eyre("Failed to compute standard deviation")?;

        Ok(Some(Stats {
            n: len + self.missing,
            missing: self.missing,
            mean,
            std: std_dev,
            // NOTE: Reason to use unwrap() is that we already tested that data
            // is not empty, and because of that it would be programming error
            // if we got None in any of the cases below. We still use '?'
            // operator we want to capture run-time error. The nearest_rank()
            // return Result<Option<Amount>, _> so if we get Err, the we want to
            // propagate, and when we get None, we should panic, because we
            // expect value, otherwise our code is incorrect.
            min: *self.data.first().unwrap(),
            p05: self.nearest_rank(dec!(0.05))?.unwrap(),
            p25: self.nearest_rank(dec!(0.25))?.unwrap(),
            p50: self.nearest_rank(dec!(0.50))?.unwrap(),
            p75: self.nearest_rank(dec!(0.75))?.unwrap(),
            p95: self.nearest_rank(dec!(0.95))?.unwrap(),
            max: *self.data.last().unwrap(),
        }))
    }
}
