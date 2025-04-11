/// abstract, allow sending orders and cancels, receiving acks, naks, executions
pub trait OrderConnector {}

#[cfg(test)]
pub mod test_util {
    use super::OrderConnector;

    pub struct MockOrderConnector {}
    impl MockOrderConnector {
        pub fn new() -> Self {
            Self {}
        }
    }
    impl OrderConnector for MockOrderConnector {}
}
