#[macro_export]
macro_rules! assert_hashmap_amounts_eq {
    ($actual:expr, $expected:expr, $tolerance:expr) => {{
        let actual: HashMap<Symbol, Amount> = $actual;
        let expected: HashMap<Symbol, Amount> = $expected;

        assert_eq!(
            actual.len(),
            expected.len(),
            "HashMaps have different lengths"
        );

        for (key, expected_value) in &expected {
            if let Some(actual_value) = actual.get(key) {
                let diff = match actual_value.checked_sub(*expected_value) {
                    Some(result) => result.abs(),
                    None => panic!("Checked subtraction failed"),
                };
                if diff > $tolerance {
                    panic!(
                        "Values for key '{}' differ by more than tolerance {} ~ {}",
                        key, actual_value, expected_value
                    );
                }
            } else {
                panic!("Key '{}' not found in actual HashMap", key);
            }
        }
    }};
}

#[cfg(test)]
mod tests {
    use crate::core::bits::{Amount, Symbol};
    use eyre::Result;
    use std::collections::HashMap;

    #[test]
    fn test_assert_hashmap_amounts_eq_passes() -> Result<()> {
        let actual: HashMap<Symbol, Amount> = [
            ("BTC".into(), "0.0417490118577075098814229249".try_into()?),
            ("ETH".into(), "1.1021739130434782608695652174".try_into()?),
        ]
        .into();

        let expected: HashMap<Symbol, Amount> = [
            ("BTC".into(), "0.0417490118577075078814229249".try_into()?),
            ("ETH".into(), "1.1021739130434782638695652174".try_into()?),
        ]
        .into();

        let tolerance: Amount = "0.0001".try_into()?;

        assert_hashmap_amounts_eq!(actual, expected, tolerance);
        Ok(())
    }

    #[test]
    #[should_panic]
    fn test_assert_hashmap_amounts_eq_wrong_value() {
        let actual: HashMap<Symbol, Amount> = [
            (
                "BTC".into(),
                "0.0417490118577075098814229249".try_into().unwrap(),
            ),
            (
                "ETH".into(),
                "1.1021739130434782608695652174".try_into().unwrap(),
            ),
        ]
        .into();

        let expected: HashMap<Symbol, Amount> = [
            (
                "BTC".into(),
                "0.9417490118577075078814229249".try_into().unwrap(),
            ),
            (
                "ETH".into(),
                "1.1721739130434782638695652174".try_into().unwrap(),
            ),
        ]
        .into();

        let tolerance: Amount = "0.0001".try_into().unwrap();

        assert_hashmap_amounts_eq!(actual, expected, tolerance);
    }

    #[test]
    #[should_panic]
    fn test_assert_hashmap_amounts_eq_wrong_key() {
        let actual: HashMap<Symbol, Amount> = [
            (
                "BTC".into(),
                "0.0417490118577075098814229249".try_into().unwrap(),
            ),
            (
                "ETH".into(),
                "1.1021739130434782608695652174".try_into().unwrap(),
            ),
        ]
        .into();

        let expected: HashMap<Symbol, Amount> = [
            (
                "BTC".into(),
                "0.0417490118577075098814229249".try_into().unwrap(),
            ),
            (
                "SOL".into(),
                "1.1021739130434782608695652174".try_into().unwrap(),
            ),
        ]
        .into();

        let tolerance: Amount = "0.0001".try_into().unwrap();

        assert_hashmap_amounts_eq!(actual, expected, tolerance);
    }

    #[test]
    #[should_panic]
    fn test_assert_hashmap_amounts_eq_wrong_length() {
        let actual: HashMap<Symbol, Amount> = [(
            "ETH".into(),
            "1.1021739130434782608695652174".try_into().unwrap(),
        )]
        .into();

        let expected: HashMap<Symbol, Amount> = [
            (
                "BTC".into(),
                "0.0417490118577075098814229249".try_into().unwrap(),
            ),
            (
                "ETH".into(),
                "1.1021739130434782608695652174".try_into().unwrap(),
            ),
        ]
        .into();

        let tolerance: Amount = "0.0001".try_into().unwrap();

        assert_hashmap_amounts_eq!(actual, expected, tolerance);
    }
}
