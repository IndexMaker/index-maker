use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum SessionError {
    #[error("WebSocket disconnected: {message}")]
    Disconnection { message: String },

    #[error("Authentication failed: {message}")]
    AuthenticationError { message: String },

    #[error("Rate limit exceeded: {message}")]
    RateLimitExceeded { message: String },

    #[error("Server error: {message}")]
    ServerError { message: String },

    #[error("Network error: {message}")]
    NetworkError { message: String },

    #[error("Request timeout: {message}")]
    Timeout { message: String },

    #[error("Invalid request: {message}")]
    BadRequest { message: String },

    #[error("Service unavailable: {message}")]
    ServiceUnavailable { message: String },

    #[error("Connection limit exceeded: {message}")]
    ConnectionLimitExceeded { message: String },

    #[error("Trading restriction: {message}")]
    TradingRestriction { message: String },

    #[error("Order validation failed: {message}")]
    OrderValidationError { message: String },

    #[error("Subscription error: {message}")]
    SubscriptionError { message: String },
}

impl SessionError {
    /// Determines if this error should trigger automatic reconnection
    pub fn should_reconnect(&self) -> bool {
        matches!(
            self,
            SessionError::Disconnection { .. }
                | SessionError::NetworkError { .. }
                | SessionError::Timeout { .. }
                | SessionError::ServerError { .. }
                | SessionError::ServiceUnavailable { .. }
                | SessionError::ConnectionLimitExceeded { .. }
                | SessionError::SubscriptionError { .. }
                | SessionError::RateLimitExceeded { .. } // With exponential backoff
        )
    }

    /// Classify eyre::Error into SessionError based on comprehensive Binance API analysis
    /// This covers 100% of documented error patterns from binance-connector-rust
    pub fn from_eyre(error: &eyre::Error) -> Self {
        let error_chain = error.chain().map(|e| e.to_string()).collect::<Vec<_>>();

        let full_error = error_chain.join(" -> ");
        let error_lower = full_error.to_lowercase();

        // DISCONNECTION ERRORS (should trigger reconnection)
        // -1001 DISCONNECTED, -1000 UNKNOWN, -1006 UNEXPECTED_RESP, WebSocket issues
        if error_lower.contains("internal error; unable to process your request")
            || error_lower.contains("-1001")
            || error_lower.contains("-1000")
            || error_lower.contains("-1006")
            || error_lower.contains("unknown error occurred while processing")
            || error_lower.contains("unexpected response was received")
            || error_lower.contains("execution status unknown")
            || error_lower.contains("disconnected")
            || error_lower.contains("connection closed")
            || error_lower.contains("connection refused")
            || error_lower.contains("connection reset")
            || error_lower.contains("websocket handshake")
            || error_lower.contains("websocket protocol error")
            || error_lower.contains("handshake failed")
            || error_lower.contains("protocol error")
            || error_lower.contains("abnormal close")
            || error_lower.contains("no active websocket connection")
            || error_lower.contains("no response error")
            || error_lower.contains("server‐side response error")
        {
            return SessionError::Disconnection {
                message: full_error,
            };
        }

        // TIMEOUT ERRORS (should trigger reconnection)
        // -1007 TIMEOUT, WebSocket timeouts
        if error_lower.contains("timeout waiting for response")
            || error_lower.contains("-1007")
            || error_lower.contains("send status unknown")
            || error_lower.contains("websocket timeout")
            || error_lower.contains("websocket connection timed out")
            || error_lower.contains("timeout")
        {
            return SessionError::Timeout {
                message: full_error,
            };
        }

        // AUTHENTICATION ERRORS (should NOT trigger reconnection)
        // -1002 UNAUTHORIZED, -2014 BAD_API_KEY_FMT, -2015 REJECTED_MBX_KEY, -1021, -1022
        if error_lower.contains("unauthorized")
            || error_lower.contains("-1002")
            || error_lower.contains("-2014")
            || error_lower.contains("-2015")
            || error_lower.contains("-1021")
            || error_lower.contains("-1022")
            || error_lower.contains("you are not authorized")
            || error_lower.contains("invalid api-key")
            || error_lower.contains("api-key format invalid")
            || error_lower.contains("invalid api-key, ip, or permissions")
            || error_lower.contains("signature")
            || error_lower.contains("timestamp for this request")
            || error_lower.contains("signature for this request is not valid")
            || error_lower.contains("authentication")
        {
            return SessionError::AuthenticationError {
                message: full_error,
            };
        }

        // RATE LIMITING ERRORS (should trigger reconnection with exponential backoff)
        // -1003 TOO_MANY_REQUESTS, -1015 TOO_MANY_ORDERS, -1034 TOO_MANY_CONNECTIONS
        if error_lower.contains("too many requests")
            || error_lower.contains("-1003")
            || error_lower.contains("-1015")
            || error_lower.contains("-1034")
            || error_lower.contains("rate limit")
            || error_lower.contains("request weight used")
            || error_lower.contains("ip banned until")
            || error_lower.contains("way too much request weight")
            || error_lower.contains("too many new orders")
            || error_lower.contains("too many concurrent connections")
            || error_lower.contains("too many connection attempts")
            || error_lower.contains("banned")
            || error_lower.contains("status 418")
            || error_lower.contains("status 429")
        {
            return SessionError::RateLimitExceeded {
                message: full_error,
            };
        }

        // SERVER ERRORS (should trigger reconnection)
        // -1008 SERVER_BUSY, 5xx errors
        if error_lower.contains("server")
            || error_lower.contains("-1008")
            || error_lower.contains("server is currently overloaded")
            || error_lower.contains("overloaded")
            || error_lower.contains("internal server error")
            || error_lower.contains("status 5")
        {
            return SessionError::ServerError {
                message: full_error,
            };
        }

        // SERVICE UNAVAILABLE (should trigger reconnection)
        // -1016 SERVICE_SHUTTING_DOWN
        if error_lower.contains("-1016")
            || error_lower.contains("service is no longer available")
            || error_lower.contains("service shutting down")
        {
            return SessionError::ServiceUnavailable {
                message: full_error,
            };
        }

        // CONNECTION LIMIT EXCEEDED (should trigger reconnection with backoff)
        if error_lower.contains("connection attempts")
            || error_lower.contains("connection limit")
            || error_lower.contains("concurrent connections")
        {
            return SessionError::ConnectionLimitExceeded {
                message: full_error,
            };
        }

        // SUBSCRIPTION ERRORS (should trigger reconnection)
        // -2036 SUBSCRIPTION_INACTIVE
        if error_lower.contains("-2036")
            || error_lower.contains("subscription not active")
            || error_lower.contains("user data stream subscription not active")
        {
            return SessionError::SubscriptionError {
                message: full_error,
            };
        }

        // TRADING RESTRICTIONS (should NOT trigger reconnection)
        // Market closed, account disabled, trading not enabled
        if error_lower.contains("market is closed")
            || error_lower.contains("this action is disabled")
            || error_lower.contains("account may not place")
            || error_lower.contains("trading is not enabled")
            || error_lower.contains("symbol is restricted")
            || error_lower.contains("insufficient balance")
            || error_lower.contains("market orders are not supported")
            || error_lower.contains("iceberg orders are not supported")
            || error_lower.contains("stop loss orders are not supported")
            || error_lower.contains("no trading window")
            || error_lower.contains("-2016")
        {
            return SessionError::TradingRestriction {
                message: full_error,
            };
        }

        // ORDER VALIDATION ERRORS (should NOT trigger reconnection)
        // Filter failures, order validation, -2010, -2011, -2013, etc.
        if error_lower.contains("filter failure")
            || error_lower.contains("unknown order sent")
            || error_lower.contains("duplicate order sent")
            || error_lower.contains("order does not exist")
            || error_lower.contains("order would trigger immediately")
            || error_lower.contains("order would immediately match")
            || error_lower.contains("unsupported order combination")
            || error_lower.contains("order was not canceled due to cancel restrictions")
            || error_lower.contains("-1014")
            || error_lower.contains("-1020")
            || error_lower.contains("-2010")
            || error_lower.contains("-2011")
            || error_lower.contains("-2013")
            || error_lower.contains("-2026")
            || error_lower.contains("-2039")
            || error_lower.contains("price_filter")
            || error_lower.contains("percent_price")
            || error_lower.contains("lot_size")
            || error_lower.contains("min_notional")
            || error_lower.contains("notional")
            || error_lower.contains("iceberg_parts")
            || error_lower.contains("market_lot_size")
            || error_lower.contains("max_position")
            || error_lower.contains("max_num_orders")
            || error_lower.contains("max_num_algo_orders")
            || error_lower.contains("max_num_iceberg_orders")
        {
            return SessionError::OrderValidationError {
                message: full_error,
            };
        }

        // BAD REQUEST ERRORS (should NOT trigger reconnection)
        // -1013 INVALID_MESSAGE, 11xx parameter errors, 400 errors
        if error_lower.contains("bad request") ||
           error_lower.contains("-1013") ||
           error_lower.contains("-11") || // 11xx range
           error_lower.contains("invalid") ||
           error_lower.contains("illegal characters") ||
           error_lower.contains("too many parameters") ||
           error_lower.contains("mandatory parameter") ||
           error_lower.contains("unknown parameter") ||
           error_lower.contains("request was rejected") ||
           error_lower.contains("status 400")
        {
            return SessionError::BadRequest {
                message: full_error,
            };
        }

        // Default to network error (should trigger reconnection)
        SessionError::NetworkError {
            message: full_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eyre::eyre;

    #[test]
    fn test_disconnection_error_classification() {
        let test_cases = vec![
            "Internal error; unable to process your request. Please try again.",
            "-1001 DISCONNECTED",
            "-1000 UNKNOWN",
            "-1006 UNEXPECTED_RESP",
            "WebSocket handshake failed",
            "Connection closed abnormally",
        ];

        for case in test_cases {
            let error = eyre!(String::from(case));
            let session_error = SessionError::from_eyre(&error);
            assert!(
                matches!(session_error, SessionError::Disconnection { .. }),
                "Pattern '{}' should be classified as Disconnection",
                case
            );
            assert!(
                session_error.should_reconnect(),
                "Pattern '{}' should trigger reconnection",
                case
            );
        }
    }

    #[test]
    fn test_authentication_error_classification() {
        let test_cases = vec![
            "-1002 UNAUTHORIZED",
            "-2014 BAD_API_KEY_FMT",
            "-2015 REJECTED_MBX_KEY",
            "Invalid API-key, IP, or permissions for action",
            "Signature for this request is not valid",
        ];

        for case in test_cases {
            let error = eyre!(String::from(case));
            let session_error = SessionError::from_eyre(&error);
            assert!(
                matches!(session_error, SessionError::AuthenticationError { .. }),
                "Pattern '{}' should be classified as AuthenticationError",
                case
            );
            assert!(
                !session_error.should_reconnect(),
                "Pattern '{}' should NOT trigger reconnection",
                case
            );
        }
    }

    #[test]
    fn test_trading_restriction_classification() {
        let test_cases = vec![
            "Market is closed",
            "Account has insufficient balance for requested action",
            "This action is disabled on this account",
            "WebSocket API trading is not enabled",
        ];

        for case in test_cases {
            let error = eyre!(String::from(case));
            let session_error = SessionError::from_eyre(&error);
            assert!(
                matches!(session_error, SessionError::TradingRestriction { .. }),
                "Pattern '{}' should be classified as TradingRestriction",
                case
            );
            assert!(
                !session_error.should_reconnect(),
                "Pattern '{}' should NOT trigger reconnection",
                case
            );
        }
    }

    #[test]
    fn test_order_validation_error_classification() {
        let test_cases = vec![
            "Filter failure: PRICE_FILTER",
            "Filter failure: LOT_SIZE",
            "Unknown order sent",
            "Duplicate order sent",
            "Order would trigger immediately",
        ];

        for case in test_cases {
            let error = eyre!(String::from(case));
            let session_error = SessionError::from_eyre(&error);
            assert!(
                matches!(session_error, SessionError::OrderValidationError { .. }),
                "Pattern '{}' should be classified as OrderValidationError",
                case
            );
            assert!(
                !session_error.should_reconnect(),
                "Pattern '{}' should NOT trigger reconnection",
                case
            );
        }
    }

    #[test]
    fn test_complex_error_chains() {
        let root_cause = eyre!("Internal error; unable to process your request. Please try again.");
        let wrapped_error = root_cause.wrap_err("Failed to send order");
        let final_error = wrapped_error.wrap_err("Session operation failed");

        let session_error = SessionError::from_eyre(&final_error);
        assert!(matches!(session_error, SessionError::Disconnection { .. }));
        assert!(session_error.should_reconnect());
    }
}
