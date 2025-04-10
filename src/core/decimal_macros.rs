#[cfg(test)]
use rust_decimal::Decimal;

#[macro_export]
macro_rules! assert_decimal_approx_eq {
    ($left:expr, $right:expr, $tolerance:expr $(,)?) => {
        match (&$left, &$right, &$tolerance) {
            (left_val, right_val, tolerance_val) => {
                let diff = match left_val.checked_sub(*right_val) {
                    Some(result) => result.abs(),
                    None => panic!("Checked subtraction failed"),
                };
                if diff > *tolerance_val {
                    panic!(
                        r#"assertion failed: `(left ≈ right) ± tolerance`
  left: `{:?}`,
 right: `{:?}`,
 diff: `{:?}`,
 tolerance: `{:?}`"#,
                        &*left_val,
                        &*right_val,
                        diff,
                        &*tolerance_val
                    )
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    #[test]
    fn test_decimal_assert_macro_passes() {
        let decimal1 = Decimal::from_str_exact("1.00000001").unwrap();
        let decimal2 = Decimal::from_str_exact("1.00000002").unwrap();
        let tolerance = Decimal::from_str_exact("0.0000001").unwrap();

        assert_decimal_approx_eq!(decimal1, decimal2, tolerance); // Should pass

        println!("Approximate equality assertions passed!");
    }

    #[test]
    #[should_panic]
    fn test_decimal_assert_macro_panics() {
        let decimal1 = Decimal::from_str_exact("1.00000001").unwrap();
        let decimal2 = Decimal::from_str_exact("1.0001").unwrap();
        let tolerance2 = Decimal::from_str_exact("0.00001").unwrap();
        assert_decimal_approx_eq!(decimal1, decimal2, tolerance2); //should panic

        println!("Approximate equality assertions passed!");
    }

}