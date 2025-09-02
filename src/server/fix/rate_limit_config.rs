use axum_fix_server::plugins::rate_limit_plugin::RateLimitConfig;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use symm_core::core::limit::LimiterConfig;

/// FIX server specific rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixRateLimitConfig {
    /// Base rate limiting configuration
    pub base: RateLimitConfig,

    /// FIX-specific message type mappings
    pub fix_message_weights: HashMap<String, usize>,

    /// Whether to send FIX NAK messages for rate limit violations
    pub send_nak_on_violation: bool,

    /// Custom retry-after header in NAK messages
    pub include_retry_after: bool,
}

impl Default for FixRateLimitConfig {
    fn default() -> Self {
        let mut fix_weights = HashMap::new();
        fix_weights.insert("NewIndexOrder".to_string(), 10);
        fix_weights.insert("CancelIndexOrder".to_string(), 5);
        fix_weights.insert("NewQuoteRequest".to_string(), 5);
        fix_weights.insert("CancelQuoteRequest".to_string(), 3);
        fix_weights.insert("AccountToCustody".to_string(), 1);
        fix_weights.insert("CustodyToAccount".to_string(), 1);
        fix_weights.insert("Heartbeat".to_string(), 1);

        Self {
            base: RateLimitConfig::default(),
            fix_message_weights: fix_weights,
            send_nak_on_violation: true,
            include_retry_after: true,
        }
    }
}

impl FixRateLimitConfig {
    /// Create conservative configuration for production
    pub fn production() -> Self {
        let mut config = Self::default();

        // More restrictive limits for production
        config.base.session_limits = vec![
            LimiterConfig::new(30, Duration::seconds(10)), // 30/10s per session
            LimiterConfig::new(500, Duration::minutes(1)), // 500/min per session
        ];

        config.base.user_limits = vec![
            LimiterConfig::new(50, Duration::seconds(10)), // 50/10s per user
            LimiterConfig::new(2000, Duration::minutes(1)), // 2000/min per user
            LimiterConfig::new(20000, Duration::days(1)),  // 20k/day per user
        ];

        config.base.global_limits = vec![
            LimiterConfig::new(500, Duration::seconds(10)), // 500/10s globally
            LimiterConfig::new(20000, Duration::minutes(1)), // 20k/min globally
        ];

        config
    }

    /// Create permissive configuration for development
    pub fn development() -> Self {
        let mut config = Self::default();

        // More permissive limits for development
        config.base.session_limits = vec![
            LimiterConfig::new(100, Duration::seconds(10)), // 100/10s per session
            LimiterConfig::new(2000, Duration::minutes(1)), // 2000/min per session
        ];

        config.base.user_limits = vec![
            LimiterConfig::new(200, Duration::seconds(10)), // 200/10s per user
            LimiterConfig::new(10000, Duration::minutes(1)), // 10k/min per user
            LimiterConfig::new(100000, Duration::days(1)),  // 100k/day per user
        ];

        config.base.global_limits = vec![
            LimiterConfig::new(2000, Duration::seconds(10)), // 2000/10s globally
            LimiterConfig::new(100000, Duration::minutes(1)), // 100k/min globally
        ];

        config
    }

    /// Disable rate limiting (for testing)
    pub fn disabled() -> Self {
        let mut config = Self::default();
        config.base.enabled = false;
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FixRateLimitConfig::default();

        // Test base configuration defaults
        assert!(config.base.enabled);
        assert_eq!(config.base.cleanup_interval_seconds, 300); // 5 minutes
        assert_eq!(config.base.max_inactive_session_age_seconds, 3600); // 1 hour as default

        // Test FIX-specific defaults
        assert!(config.send_nak_on_violation);
        assert!(config.include_retry_after);

        // Test message weight mappings
        assert_eq!(config.fix_message_weights.get("NewIndexOrder"), Some(&10));
        assert_eq!(config.fix_message_weights.get("CancelIndexOrder"), Some(&5));
        assert_eq!(config.fix_message_weights.get("NewQuoteRequest"), Some(&5));
        assert_eq!(
            config.fix_message_weights.get("CancelQuoteRequest"),
            Some(&3)
        );
        assert_eq!(config.fix_message_weights.get("AccountToCustody"), Some(&1));
        assert_eq!(config.fix_message_weights.get("CustodyToAccount"), Some(&1));
        assert_eq!(config.fix_message_weights.get("Heartbeat"), Some(&1));

        // Test that all expected message types are present
        assert_eq!(config.fix_message_weights.len(), 7);
    }

    #[test]
    fn test_production_config() {
        let config = FixRateLimitConfig::production();

        // Test that production config is more restrictive
        assert!(config.base.enabled);

        // Test session limits (should be more restrictive than default)
        assert_eq!(config.base.session_limits.len(), 2);
        assert_eq!(config.base.session_limits[0].limit, 30);
        assert_eq!(config.base.session_limits[0].period, Duration::seconds(10));
        assert_eq!(config.base.session_limits[1].limit, 500);
        assert_eq!(config.base.session_limits[1].period, Duration::minutes(1));

        // Test user limits
        assert_eq!(config.base.user_limits.len(), 3);
        assert_eq!(config.base.user_limits[0].limit, 50);
        assert_eq!(config.base.user_limits[0].period, Duration::seconds(10));
        assert_eq!(config.base.user_limits[1].limit, 2000);
        assert_eq!(config.base.user_limits[1].period, Duration::minutes(1));
        assert_eq!(config.base.user_limits[2].limit, 20000);
        assert_eq!(config.base.user_limits[2].period, Duration::days(1));

        // Test global limits
        assert_eq!(config.base.global_limits.len(), 2);
        assert_eq!(config.base.global_limits[0].limit, 500);
        assert_eq!(config.base.global_limits[0].period, Duration::seconds(10));
        assert_eq!(config.base.global_limits[1].limit, 20000);
        assert_eq!(config.base.global_limits[1].period, Duration::minutes(1));

        // Test FIX-specific settings remain the same
        assert!(config.send_nak_on_violation);
        assert!(config.include_retry_after);
    }

    #[test]
    fn test_development_config() {
        let config = FixRateLimitConfig::development();

        // Test that development config is more permissive
        assert!(config.base.enabled);

        // Test session limits (should be more permissive than production)
        assert_eq!(config.base.session_limits.len(), 2);
        assert_eq!(config.base.session_limits[0].limit, 100);
        assert_eq!(config.base.session_limits[0].period, Duration::seconds(10));
        assert_eq!(config.base.session_limits[1].limit, 2000);
        assert_eq!(config.base.session_limits[1].period, Duration::minutes(1));

        // Test user limits
        assert_eq!(config.base.user_limits.len(), 3);
        assert_eq!(config.base.user_limits[0].limit, 200);
        assert_eq!(config.base.user_limits[0].period, Duration::seconds(10));
        assert_eq!(config.base.user_limits[1].limit, 10000);
        assert_eq!(config.base.user_limits[1].period, Duration::minutes(1));
        assert_eq!(config.base.user_limits[2].limit, 100000);
        assert_eq!(config.base.user_limits[2].period, Duration::days(1));

        // Test global limits
        assert_eq!(config.base.global_limits.len(), 2);
        assert_eq!(config.base.global_limits[0].limit, 2000);
        assert_eq!(config.base.global_limits[0].period, Duration::seconds(10));
        assert_eq!(config.base.global_limits[1].limit, 100000);
        assert_eq!(config.base.global_limits[1].period, Duration::minutes(1));
    }

    #[test]
    fn test_disabled_config() {
        let config = FixRateLimitConfig::disabled();

        // Test that rate limiting is disabled
        assert!(!config.base.enabled);

        // Other settings should still be present
        assert!(config.send_nak_on_violation);
        assert!(config.include_retry_after);
        assert!(!config.fix_message_weights.is_empty());
    }

    #[test]
    fn test_config_comparison() {
        let default_config = FixRateLimitConfig::default();
        let production_config = FixRateLimitConfig::production();
        let development_config = FixRateLimitConfig::development();

        // Production should be more restrictive than development for session limits
        assert!(
            production_config.base.session_limits[0].limit
                < development_config.base.session_limits[0].limit
        );
        assert!(
            production_config.base.user_limits[0].limit
                < development_config.base.user_limits[0].limit
        );
        assert!(
            production_config.base.global_limits[0].limit
                < development_config.base.global_limits[0].limit
        );

        // All configs should have the same FIX message weights
        assert_eq!(
            default_config.fix_message_weights,
            production_config.fix_message_weights
        );
        assert_eq!(
            default_config.fix_message_weights,
            development_config.fix_message_weights
        );
    }

    #[test]
    fn test_message_weight_lookup() {
        let config = FixRateLimitConfig::default();

        // Test that we can look up weights for all expected message types
        let expected_weights = vec![
            ("NewIndexOrder", 10),
            ("CancelIndexOrder", 5),
            ("NewQuoteRequest", 5),
            ("CancelQuoteRequest", 3),
            ("AccountToCustody", 1),
            ("CustodyToAccount", 1),
            ("Heartbeat", 1),
        ];

        for (msg_type, expected_weight) in expected_weights {
            assert_eq!(
                config.fix_message_weights.get(msg_type),
                Some(&expected_weight),
                "Message type {} should have weight {}",
                msg_type,
                expected_weight
            );
        }

        // Test that unknown message types return None
        assert_eq!(config.fix_message_weights.get("UnknownMessageType"), None);
    }
}
