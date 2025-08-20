use eyre::Error;
use serde::{Deserialize, Serialize};

/// Classifies errors to determine if they indicate WebSocket disconnection
pub struct ErrorClassifier;

impl ErrorClassifier {
    /// Determines if an error indicates a WebSocket disconnection
    pub fn is_disconnection_error(error: &Error) -> bool {
        // Check the entire error chain for -1001 DISCONNECTED
        for err in error.chain() {
            let error_str = err.to_string().to_lowercase();
            
            if error_str.contains("internal error; unable to process your request. please try again") {
                return true;
            }
        }
        false
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use eyre::eyre;
    use test_case::test_case;

    #[test_case("Internal error; unable to process your request. Please try again." => true; "binance_1001")]
    #[test_case("Invalid API key" => false; "not_disconnection")]
    #[test_case("Rate limit exceeded" => false; "not_disconnection_2")]
    fn test_is_disconnection_error(error_message: &str) -> bool {
        let error = eyre!(error_message.to_string());
        ErrorClassifier::is_disconnection_error(&error)
    }

    #[test]
    fn test_complex_error_chains() {
        let root_cause = eyre!("Internal error; unable to process your request. Please try again.");
        let wrapped_error = root_cause.wrap_err("Failed to send order");
        let final_error = wrapped_error.wrap_err("Session operation failed");
        
        assert!(ErrorClassifier::is_disconnection_error(&final_error));
    }
}