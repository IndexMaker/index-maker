#[macro_export]
macro_rules! assert_deque_single_matches {
    ($deque:expr, $pattern:pat if $condition:expr) => {
        {
            let mut deque = $deque;
            assert_eq!(deque.len(), 1);
            assert!(matches!(deque.pop_back(), Some($pattern) if $condition));
        }
    };
    ($deque:expr, $pattern:pat) => {
        {
            let mut deque = $deque;
            assert_eq!(deque.len(), 1);
            assert!(matches!(deque.pop_back(), Some($pattern)));
        }
    };
}

#[macro_export]
macro_rules! assert_deque_single_matches_and_return {
    ($deque:expr, $pattern:pat if $condition:expr) => {
        {
            let mut deque = $deque;
            assert_eq!(deque.len(), 1);
            if let Some(val) = deque.pop_back() {
                assert!(matches!(&val, $pattern if $condition));
                Some(val)
            } else {
                panic!("Deque was empty, despite length check.");
            }
        }
    };
    ($deque:expr, $pattern:pat) => {
        {
            let mut deque = $deque;
            assert_eq!(deque.len(), 1);
            if let Some(val) = deque.pop_back() {
                assert!(matches!(&val, $pattern));
                Some(val)
            } else {
                panic!("Deque was empty, despite length check.");
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    #[test]
    fn test_assert_deque_single_matches_and_return() {
        let mut deque = VecDeque::new();
        deque.push_back("hello");

        if let Some(x) = assert_deque_single_matches_and_return!(deque, &"hello") {
            assert_eq!(x, "hello");
        }
    }

    #[test]
    fn test_assert_deque_single_matches_and_return_with_condition() {
        let mut deque = VecDeque::new();
        deque.push_back(10);

        if let Some(x) = assert_deque_single_matches_and_return!(deque, 10 if 10 == 10) {
            assert_eq!(x, 10);
        }
    }

    #[test]
    #[should_panic]
    fn test_assert_deque_single_matches_and_return_wrong_length() {
        let mut deque = VecDeque::new();
        deque.push_back(10);
        deque.push_back(20);

        assert_deque_single_matches_and_return!(deque, 10);
    }

    #[test]
    #[should_panic]
    fn test_assert_deque_single_matches_and_return_wrong_value() {
        let mut deque = VecDeque::new();
        deque.push_back(10);

        assert_deque_single_matches_and_return!(deque, 20);
    }

    #[test]
    #[should_panic]
    fn test_assert_deque_single_matches_and_return_wrong_condition() {
        let mut deque = VecDeque::new();
        deque.push_back(10);

        assert_deque_single_matches_and_return!(deque, 10 if 10 == 11);
    }
}