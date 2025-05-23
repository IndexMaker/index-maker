#[macro_export]
macro_rules! test_case {
    (
        name: $test_name:literal, // Name of the test case, e.g., "Unlimited"
        title: $test_title:literal, // Name of the test case, e.g., "Unlimited"
        runner: $test_runner:ident, // Specify the runner function by name
        config: ( $($config_item:expr),* ), // The tuple for the config
        expected: [ $( ($($expected_inner:expr),*) ),* ] // The Vec of expected tuples
    ) => {
        // This generates a unique test function for each call to test_case!
        // We sanitize the name to create a valid Rust function name
        paste::item! { // Requires the `paste` crate
            #[test]
            fn [<test_ $test_name:snake>]() { // Generates fn test_unlimited() for "Unlimited"
                $test_runner(
                    $test_title,
                    ( $($config_item),* ),
                    vec![ $( ($($expected_inner),*) ),* ],
                );
            }
        }
    };
}