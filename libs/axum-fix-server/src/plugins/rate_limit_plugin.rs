use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use eyre::{eyre, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use symm_core::core::{
    bits::Address,
    limit::{LimiterConfig, MultiLimiter},
};
use tokio::time::{interval, Duration as TokioDuration};

use crate::messages::SessionId;
use crate::plugins::user_plugin::WithUserPlugin;

/// Core rate limiting plugin for FIX server
pub struct RateLimitPlugin<R, Q> {
    /// Per-session rate limiters
    session_limiters: Arc<RwLock<HashMap<SessionId, Arc<RwLock<MultiLimiter>>>>>,

    /// Per-user rate limiters
    user_limiters: Arc<RwLock<HashMap<(u32, Address), Arc<RwLock<MultiLimiter>>>>>,

    /// Global rate limiter
    global_limiter: Arc<RwLock<MultiLimiter>>,

    /// Configuration
    config: RateLimitConfig,

    /// Metrics for monitoring
    metrics: Arc<RateLimitMetrics>,

    /// Session metadata for cleanup
    session_metadata: Arc<RwLock<HashMap<SessionId, SessionMetadata>>>,

    /// User metadata for cleanup
    user_metadata: Arc<RwLock<HashMap<(u32, Address), UserMetadata>>>,

    /// Phantom types for trait compliance
    _phantom_r: PhantomData<R>,
    _phantom_q: PhantomData<Q>,
}

/// Configuration for rate limiting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Session-level rate limits
    pub session_limits: Vec<LimiterConfig>,

    /// User-level rate limits
    pub user_limits: Vec<LimiterConfig>,

    /// Global rate limits
    pub global_limits: Vec<LimiterConfig>,

    /// Message type weights
    pub message_type_weights: HashMap<MessageType, usize>,

    /// Cleanup interval in seconds
    pub cleanup_interval_seconds: u64,

    /// Maximum age for inactive sessions in seconds
    pub max_inactive_session_age_seconds: u64,

    /// Whether rate limiting is enabled
    pub enabled: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        let mut message_weights = HashMap::new();
        message_weights.insert(MessageType::Order, 10);
        message_weights.insert(MessageType::Quote, 5);
        message_weights.insert(MessageType::Administrative, 1);
        message_weights.insert(MessageType::Heartbeat, 1);

        Self {
            session_limits: vec![
                LimiterConfig::new(50, Duration::seconds(10)), // 50/10s per session
                LimiterConfig::new(1000, Duration::minutes(1)), // 1000/min per session
            ],
            user_limits: vec![
                LimiterConfig::new(100, Duration::seconds(10)), // 100/10s per user
                LimiterConfig::new(5000, Duration::minutes(1)), // 5000/min per user
                LimiterConfig::new(50000, Duration::days(1)),   // 50k/day per user
            ],
            global_limits: vec![
                LimiterConfig::new(1000, Duration::seconds(10)), // 1000/10s globally
                LimiterConfig::new(50000, Duration::minutes(1)), // 50k/min globally
            ],
            message_type_weights: message_weights,
            cleanup_interval_seconds: 300,          // 5 minutes
            max_inactive_session_age_seconds: 3600, // 1 hour
            enabled: true,
        }
    }
}

/// Message types for rate limiting
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum MessageType {
    Order,
    Quote,
    Administrative,
    Heartbeat,
}

/// Rate limiting metrics
#[derive(Debug, Default)]
pub struct RateLimitMetrics {
    pub total_requests: AtomicU64,
    pub rejected_requests: AtomicU64,
    pub active_sessions: AtomicU64,
    pub active_users: AtomicU64,
}

/// Session metadata for cleanup
#[derive(Debug, Clone)]
struct SessionMetadata {
    last_activity: DateTime<Utc>,
    user_id: (u32, Address),
    total_requests: u64,
}

/// User metadata for cleanup
#[derive(Debug, Clone)]
struct UserMetadata {
    last_activity: DateTime<Utc>,
    active_sessions: std::collections::HashSet<SessionId>,
    total_requests: u64,
}

/// Rate limiting errors
#[derive(Debug, thiserror::Error)]
pub enum RateLimitError {
    #[error("Global rate limit exceeded. Retry after: {retry_after:?}")]
    GlobalLimitExceeded { retry_after: Duration },

    #[error("Session rate limit exceeded for {session_id}. Retry after: {retry_after:?}")]
    SessionLimitExceeded {
        session_id: SessionId,
        retry_after: Duration,
    },

    #[error("User rate limit exceeded for {user_id:?} on {message_type:?} (weight: {weight}). Retry after: {retry_after:?}")]
    UserLimitExceeded {
        user_id: (u32, Address),
        message_type: MessageType,
        weight: usize,
        retry_after: Duration,
    },
}

/// Trait for types that can be rate limited
pub trait WithRateLimitPlugin {
    fn get_rate_limit_key(&self) -> RateLimitKey;
    fn get_message_weight(&self) -> usize;
    fn get_message_type(&self) -> MessageType;
    fn get_user_id(&self) -> (u32, Address);
}

/// Rate limiting key types
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum RateLimitKey {
    Session(SessionId),
    User(u32, Address),
    Global,
}

impl<R, Q> RateLimitPlugin<R, Q>
where
    R: Send + Sync + WithRateLimitPlugin,
    Q: Send + Sync + WithRateLimitPlugin,
{
    /// Create a new rate limiting plugin
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            session_limiters: Arc::new(RwLock::new(HashMap::new())),
            user_limiters: Arc::new(RwLock::new(HashMap::new())),
            global_limiter: Arc::new(RwLock::new(MultiLimiter::new(config.global_limits.clone()))),
            session_metadata: Arc::new(RwLock::new(HashMap::new())),
            user_metadata: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RateLimitMetrics::default()),
            config,
            _phantom_r: PhantomData,
            _phantom_q: PhantomData,
        }
    }

    /// Check rate limits before processing a message
    pub fn check_rate_limits(
        &self,
        session_id: &SessionId,
        user_id: &(u32, Address),
    ) -> Result<(), RateLimitError> {
        if !self.config.enabled {
            return Ok(());
        }

        let timestamp = Utc::now();

        // Increment total requests
        self.metrics.total_requests.fetch_add(1, Ordering::Relaxed);

        // 1. Global limit check (highest priority)
        if !self.global_limiter.write().try_consume(1, timestamp) {
            self.metrics
                .rejected_requests
                .fetch_add(1, Ordering::Relaxed);
            return Err(RateLimitError::GlobalLimitExceeded {
                retry_after: self
                    .global_limiter
                    .read()
                    .waiting_period_half_smallest_limit(timestamp),
            });
        }

        // 2. Session limit check
        let session_limiter = self.get_or_create_session_limiter(session_id);
        if !session_limiter.write().try_consume(1, timestamp) {
            self.metrics
                .rejected_requests
                .fetch_add(1, Ordering::Relaxed);
            return Err(RateLimitError::SessionLimitExceeded {
                session_id: session_id.clone(),
                retry_after: session_limiter
                    .read()
                    .waiting_period_half_smallest_limit(timestamp),
            });
        }

        // 3. User limit check
        let user_limiter = self.get_or_create_user_limiter(user_id);
        if !user_limiter.write().try_consume(1, timestamp) {
            self.metrics
                .rejected_requests
                .fetch_add(1, Ordering::Relaxed);
            return Err(RateLimitError::UserLimitExceeded {
                user_id: *user_id,
                message_type: MessageType::Administrative, // Default, will be updated
                weight: 1,
                retry_after: user_limiter
                    .read()
                    .waiting_period_half_smallest_limit(timestamp),
            });
        }

        // Update activity timestamps
        self.update_activity_timestamps(session_id, user_id, timestamp);

        Ok(())
    }

    /// Check rate limits for a specific message with weight
    pub fn consume_message_weight(
        &self,
        request: &R,
        _session_id: &SessionId,
    ) -> Result<(), RateLimitError> {
        if !self.config.enabled {
            return Ok(());
        }

        let timestamp = Utc::now();
        let message_type = request.get_message_type();
        let weight = self
            .config
            .message_type_weights
            .get(&message_type)
            .copied()
            .unwrap_or(1);

        let user_id = request.get_user_id();

        // Check user limits with message weight
        let user_limiter = self.get_or_create_user_limiter(&user_id);
        if !user_limiter.write().try_consume(weight, timestamp) {
            self.metrics
                .rejected_requests
                .fetch_add(1, Ordering::Relaxed);
            return Err(RateLimitError::UserLimitExceeded {
                user_id,
                message_type,
                weight,
                retry_after: user_limiter
                    .read()
                    .waiting_period_half_smallest_limit(timestamp),
            });
        }

        Ok(())
    }

    /// Get or create session limiter
    fn get_or_create_session_limiter(&self, session_id: &SessionId) -> Arc<RwLock<MultiLimiter>> {
        // Fast path: read-only access
        {
            let session_limiters = self.session_limiters.read();
            if let Some(limiter) = session_limiters.get(session_id) {
                return limiter.clone();
            }
        }

        // Slow path: create new limiter
        let mut session_limiters = self.session_limiters.write();

        // Double-check pattern
        if let Some(limiter) = session_limiters.get(session_id) {
            return limiter.clone();
        }

        let new_limiter = Arc::new(RwLock::new(MultiLimiter::new(
            self.config.session_limits.clone(),
        )));

        session_limiters.insert(session_id.clone(), new_limiter.clone());
        self.metrics.active_sessions.fetch_add(1, Ordering::Relaxed);

        new_limiter
    }

    /// Get or create user limiter
    fn get_or_create_user_limiter(&self, user_id: &(u32, Address)) -> Arc<RwLock<MultiLimiter>> {
        // Fast path: read-only access
        {
            let user_limiters = self.user_limiters.read();
            if let Some(limiter) = user_limiters.get(user_id) {
                return limiter.clone();
            }
        }

        // Slow path: create new limiter
        let mut user_limiters = self.user_limiters.write();

        // Double-check pattern
        if let Some(limiter) = user_limiters.get(user_id) {
            return limiter.clone();
        }

        let new_limiter = Arc::new(RwLock::new(MultiLimiter::new(
            self.config.user_limits.clone(),
        )));

        user_limiters.insert(*user_id, new_limiter.clone());
        self.metrics.active_users.fetch_add(1, Ordering::Relaxed);

        new_limiter
    }

    /// Update activity timestamps
    fn update_activity_timestamps(
        &self,
        session_id: &SessionId,
        user_id: &(u32, Address),
        timestamp: DateTime<Utc>,
    ) {
        // Update session metadata
        {
            let mut metadata = self.session_metadata.write();
            metadata
                .entry(session_id.clone())
                .and_modify(|meta| {
                    meta.last_activity = timestamp;
                    meta.total_requests += 1;
                })
                .or_insert(SessionMetadata {
                    last_activity: timestamp,
                    user_id: *user_id,
                    total_requests: 1,
                });
        }

        // Update user metadata
        {
            let mut metadata = self.user_metadata.write();
            metadata
                .entry(*user_id)
                .and_modify(|meta| {
                    meta.last_activity = timestamp;
                    meta.total_requests += 1;
                    meta.active_sessions.insert(session_id.clone());
                })
                .or_insert_with(|| {
                    let mut sessions = std::collections::HashSet::new();
                    sessions.insert(session_id.clone());
                    UserMetadata {
                        last_activity: timestamp,
                        active_sessions: sessions,
                        total_requests: 1,
                    }
                });
        }
    }

    /// Create session
    pub fn create_session(&self, session_id: &SessionId) -> Result<()> {
        let timestamp = Utc::now();

        // Initialize session metadata
        let mut metadata = self.session_metadata.write();
        metadata.insert(
            session_id.clone(),
            SessionMetadata {
                last_activity: timestamp,
                user_id: (0, Address::ZERO), // Will be updated on first request
                total_requests: 0,
            },
        );

        Ok(())
    }

    /// Destroy session
    pub fn destroy_session(&self, session_id: &SessionId) -> Result<()> {
        // Remove session limiter
        self.session_limiters.write().remove(session_id);

        // Remove session metadata and update user metadata
        if let Some(meta) = self.session_metadata.write().remove(session_id) {
            let mut user_metadata = self.user_metadata.write();
            if let Some(user_meta) = user_metadata.get_mut(&meta.user_id) {
                user_meta.active_sessions.remove(session_id);
                if user_meta.active_sessions.is_empty() {
                    user_metadata.remove(&meta.user_id);
                    self.metrics.active_users.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }

        self.metrics.active_sessions.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }

    /// Get current metrics
    pub fn get_metrics(&self) -> RateLimitMetricsSnapshot {
        RateLimitMetricsSnapshot {
            total_requests: self.metrics.total_requests.load(Ordering::Relaxed),
            rejected_requests: self.metrics.rejected_requests.load(Ordering::Relaxed),
            active_sessions: self.session_limiters.read().len() as u64,
            active_users: self.user_limiters.read().len() as u64,
        }
    }

    /// Start cleanup task
    pub fn start_cleanup_task(&self) -> tokio::task::JoinHandle<()> {
        let config = self.config.clone();

        let session_limiters = self.session_limiters.clone();
        let user_limiters = self.user_limiters.clone();
        let session_metadata = self.session_metadata.clone();
        let user_metadata = self.user_metadata.clone();
        let metrics = self.metrics.clone();

        tokio::spawn(async move {
            let mut cleanup_interval =
                interval(TokioDuration::from_secs(config.cleanup_interval_seconds));

            loop {
                cleanup_interval.tick().await;

                let now = Utc::now();
                let max_age = Duration::seconds(config.max_inactive_session_age_seconds as i64);

                Self::cleanup_inactive_sessions(
                    &session_limiters,
                    &user_limiters,
                    &session_metadata,
                    &user_metadata,
                    &metrics,
                    now,
                    max_age,
                )
                .await;
            }
        })
    }

    async fn cleanup_inactive_sessions(
        session_limiters: &Arc<RwLock<HashMap<SessionId, Arc<RwLock<MultiLimiter>>>>>,
        user_limiters: &Arc<RwLock<HashMap<(u32, Address), Arc<RwLock<MultiLimiter>>>>>,
        session_metadata: &Arc<RwLock<HashMap<SessionId, SessionMetadata>>>,
        user_metadata: &Arc<RwLock<HashMap<(u32, Address), UserMetadata>>>,
        metrics: &Arc<RateLimitMetrics>,
        now: DateTime<Utc>,
        max_age: Duration,
    ) {
        let cutoff_time = now - max_age;
        let mut sessions_to_remove = Vec::new();

        // Identify inactive sessions
        {
            let metadata = session_metadata.read();
            for (session_id, meta) in metadata.iter() {
                if meta.last_activity < cutoff_time {
                    sessions_to_remove.push(session_id.clone());
                }
            }
        }

        // Remove inactive sessions
        if !sessions_to_remove.is_empty() {
            let mut s_limiters = session_limiters.write();
            let mut s_metadata = session_metadata.write();
            let mut u_metadata = user_metadata.write();

            for session_id in &sessions_to_remove {
                s_limiters.remove(session_id);
                if let Some(meta) = s_metadata.remove(session_id) {
                    if let Some(user_meta) = u_metadata.get_mut(&meta.user_id) {
                        user_meta.active_sessions.remove(session_id);
                        if user_meta.active_sessions.is_empty() {
                            u_metadata.remove(&meta.user_id);
                            metrics.active_users.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                }
                metrics.active_sessions.fetch_sub(1, Ordering::Relaxed);
            }

            tracing::info!("Cleaned up {} inactive sessions", sessions_to_remove.len());
        }
    }
}

/// Snapshot of rate limiting metrics
#[derive(Debug, Clone)]
pub struct RateLimitMetricsSnapshot {
    pub total_requests: u64,
    pub rejected_requests: u64,
    pub active_sessions: u64,
    pub active_users: u64,
}

impl Clone for RateLimitMetrics {
    fn clone(&self) -> Self {
        Self {
            total_requests: AtomicU64::new(self.total_requests.load(Ordering::Relaxed)),
            rejected_requests: AtomicU64::new(self.rejected_requests.load(Ordering::Relaxed)),
            active_sessions: AtomicU64::new(self.active_sessions.load(Ordering::Relaxed)),
            active_users: AtomicU64::new(self.active_users.load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::collections::HashMap;
    use symm_core::core::{
        limit::LimiterConfig,
        test_util::{get_mock_address_1, get_mock_address_2},
    };

    // Mock request type for testing
    #[derive(Debug, Clone)]
    struct TestRequest {
        user_id: (u32, Address),
        message_type: MessageType,
        weight: usize,
    }

    impl WithRateLimitPlugin for TestRequest {
        fn get_rate_limit_key(&self) -> RateLimitKey {
            RateLimitKey::User(self.user_id.0, self.user_id.1)
        }

        fn get_message_weight(&self) -> usize {
            self.weight
        }

        fn get_message_type(&self) -> MessageType {
            self.message_type.clone()
        }

        fn get_user_id(&self) -> (u32, Address) {
            self.user_id
        }
    }

    // Mock response type for testing
    #[derive(Debug, Clone)]
    struct TestResponse {
        user_id: (u32, Address),
        message_type: MessageType,
    }

    impl WithRateLimitPlugin for TestResponse {
        fn get_rate_limit_key(&self) -> RateLimitKey {
            RateLimitKey::User(self.user_id.0, self.user_id.1)
        }

        fn get_message_weight(&self) -> usize {
            match self.message_type {
                MessageType::Order => 3,
                MessageType::Quote => 2,
                _ => 1,
            }
        }

        fn get_message_type(&self) -> MessageType {
            self.message_type.clone()
        }

        fn get_user_id(&self) -> (u32, Address) {
            self.user_id
        }
    }

    impl<R, S> RateLimitPlugin<R, S>
    where
        R: Send + Sync + WithRateLimitPlugin,
        S: Send + Sync + WithRateLimitPlugin,
    {
        #[cfg(test)]
        pub fn consume_message_weight_at_time(
            &self,
            request: &R,
            _session_id: &SessionId,
            timestamp: DateTime<Utc>,
        ) -> Result<(), RateLimitError> {
            if !self.config.enabled {
                return Ok(());
            }

            let message_type = request.get_message_type();
            let weight = self
                .config
                .message_type_weights
                .get(&message_type)
                .copied()
                .unwrap_or(1);

            let user_id = request.get_user_id();

            // Check user limits with explicit timestamp
            let user_limiter = self.get_or_create_user_limiter(&user_id);
            if !user_limiter.write().try_consume(weight, timestamp) {
                self.metrics
                    .rejected_requests
                    .fetch_add(1, Ordering::Relaxed);
                return Err(RateLimitError::UserLimitExceeded {
                    user_id,
                    message_type,
                    weight,
                    retry_after: user_limiter
                        .read()
                        .waiting_period_half_smallest_limit(timestamp),
                });
            }

            Ok(())
        }

        #[cfg(test)]
        pub fn check_rate_limits_at_time(
            &self,
            session_id: &SessionId,
            user_id: &(u32, Address),
            timestamp: DateTime<Utc>,
        ) -> Result<(), RateLimitError> {
            if !self.config.enabled {
                return Ok(());
            }

            // Increment total requests
            self.metrics.total_requests.fetch_add(1, Ordering::Relaxed);

            // 1. Global limit check
            if !self.global_limiter.write().try_consume(1, timestamp) {
                self.metrics
                    .rejected_requests
                    .fetch_add(1, Ordering::Relaxed);
                return Err(RateLimitError::GlobalLimitExceeded {
                    retry_after: self
                        .global_limiter
                        .read()
                        .waiting_period_half_smallest_limit(timestamp),
                });
            }

            // 2. Session limit check
            let session_limiter = self.get_or_create_session_limiter(session_id);
            if !session_limiter.write().try_consume(1, timestamp) {
                self.metrics
                    .rejected_requests
                    .fetch_add(1, Ordering::Relaxed);
                return Err(RateLimitError::SessionLimitExceeded {
                    session_id: session_id.clone(),
                    retry_after: session_limiter
                        .read()
                        .waiting_period_half_smallest_limit(timestamp),
                });
            }

            // 3. User limit check
            let user_limiter = self.get_or_create_user_limiter(user_id);
            if !user_limiter.write().try_consume(1, timestamp) {
                self.metrics
                    .rejected_requests
                    .fetch_add(1, Ordering::Relaxed);
                return Err(RateLimitError::UserLimitExceeded {
                    user_id: *user_id,
                    message_type: MessageType::Administrative, // Default for rate limit checks
                    weight: 1,
                    retry_after: user_limiter
                        .read()
                        .waiting_period_half_smallest_limit(timestamp),
                });
            }

            Ok(())
        }
    }

    fn create_test_config() -> RateLimitConfig {
        let mut message_weights = HashMap::new();
        message_weights.insert(MessageType::Order, 5);
        message_weights.insert(MessageType::Quote, 3);
        message_weights.insert(MessageType::Administrative, 1);

        RateLimitConfig {
            session_limits: vec![LimiterConfig::new(5, Duration::seconds(60))],
            user_limits: vec![LimiterConfig::new(10, Duration::seconds(60))],
            global_limits: vec![LimiterConfig::new(20, Duration::seconds(60))],
            message_type_weights: message_weights,
            cleanup_interval_seconds: 60,
            max_inactive_session_age_seconds: 120,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn test_rate_limit_config_creation() {
        let config = create_test_config();
        assert!(config.enabled);
        assert_eq!(config.session_limits.len(), 1);
        assert_eq!(config.user_limits.len(), 1);
        assert_eq!(config.global_limits.len(), 1);
        assert_eq!(config.message_type_weights.len(), 3);
    }

    #[tokio::test]
    async fn test_rate_limit_plugin_creation() {
        let config = create_test_config();
        let plugin: RateLimitPlugin<TestRequest, TestResponse> = RateLimitPlugin::new(config);

        let metrics = plugin.get_metrics();
        assert_eq!(metrics.total_requests, 0);
        assert_eq!(metrics.rejected_requests, 0);
        assert_eq!(metrics.active_sessions, 0);
        assert_eq!(metrics.active_users, 0);
    }

    #[tokio::test]
    async fn test_rate_limit_enforcement() {
        let config = create_test_config();
        let plugin: RateLimitPlugin<TestRequest, TestResponse> = RateLimitPlugin::new(config);

        let session_id = SessionId::from("test_session");
        let user_id = (1, get_mock_address_1());

        // Create session
        plugin.create_session(&session_id).unwrap();

        // Should allow up to 5 requests (session limit)
        for i in 0..5 {
            let result = plugin.check_rate_limits(&session_id, &user_id);
            assert!(result.is_ok(), "Request {} should be allowed", i);
        }

        // 6th request should be rejected
        let result = plugin.check_rate_limits(&session_id, &user_id);
        assert!(result.is_err());

        if let Err(RateLimitError::SessionLimitExceeded {
            session_id: err_session,
            ..
        }) = result
        {
            assert_eq!(err_session, session_id);
        } else {
            panic!("Expected SessionLimitExceeded error");
        }
    }

    #[tokio::test]
    async fn test_message_weight_consumption() {
        let config = create_test_config();
        let plugin: RateLimitPlugin<TestRequest, TestResponse> = RateLimitPlugin::new(config);

        let session_id = SessionId::from("weight_test");
        let user_id = (1, get_mock_address_1());

        plugin.create_session(&session_id).unwrap();

        let mut timestamp = Utc::now();

        // Create request with weight 10 (should consume all user limit)
        let request = TestRequest {
            user_id,
            message_type: MessageType::Order,
            weight: 10,
        };

        // ✅ Test 1: First request should succeed (5/10 used)
        let result = plugin.consume_message_weight_at_time(&request, &session_id, timestamp);
        assert!(result.is_ok(), "First request should succeed");

        // ✅ Test 2: Second request should succeed (10/10 used)
        timestamp += chrono::TimeDelta::milliseconds(100); // Same window
        let result = plugin.consume_message_weight_at_time(&request, &session_id, timestamp);
        assert!(result.is_ok(), "Second request should succeed");

        // ✅ Test 3: Third request should fail (15/10 would exceed limit)
        timestamp += chrono::TimeDelta::milliseconds(100); // Same window
        let result = plugin.consume_message_weight_at_time(&request, &session_id, timestamp);
        assert!(
            result.is_err(),
            "Third request should fail due to weight limit"
        );

        // ✅ Test 4: Verify error type
        match result {
            Err(RateLimitError::UserLimitExceeded {
                user_id: err_user,
                weight,
                ..
            }) => {
                assert_eq!(err_user, user_id);
                assert_eq!(weight, 5); // Config weight, not request weight
            }
            _ => panic!("Expected UserLimitExceeded error"),
        }
    }

    #[tokio::test]
    async fn test_session_management() {
        let config = create_test_config();
        let plugin: RateLimitPlugin<TestRequest, TestResponse> = RateLimitPlugin::new(config);

        let session_id = SessionId::from("session_mgmt_test");

        // Initially no sessions
        assert_eq!(plugin.get_metrics().active_sessions, 0);

        // Create session
        plugin.create_session(&session_id).unwrap();

        // Destroy session
        plugin.destroy_session(&session_id).unwrap();

        // Should be back to 0 sessions
        let metrics = plugin.get_metrics();
        assert_eq!(metrics.active_sessions, 0);
    }

    #[tokio::test]
    async fn test_disabled_rate_limiting() {
        let mut config = create_test_config();
        config.enabled = false;

        let plugin: RateLimitPlugin<TestRequest, TestResponse> = RateLimitPlugin::new(config);

        let session_id = SessionId::from("disabled_test");
        let user_id = (1, get_mock_address_1());

        plugin.create_session(&session_id).unwrap();

        // Should allow unlimited requests when disabled
        for _ in 0..100 {
            assert!(plugin.check_rate_limits(&session_id, &user_id).is_ok());
        }
    }

    #[tokio::test]
    async fn test_rate_limit_key_types() {
        let user_id = (1, get_mock_address_1());
        let session_id = SessionId::from("key_test_session");

        let session_key = RateLimitKey::Session(session_id.clone());
        let user_key = RateLimitKey::User(user_id.0, user_id.1);
        let global_key = RateLimitKey::Global;

        // Test equality and hashing work correctly
        assert_eq!(session_key, RateLimitKey::Session(session_id));
        assert_eq!(user_key, RateLimitKey::User(user_id.0, user_id.1));
        assert_eq!(global_key, RateLimitKey::Global);

        assert_ne!(session_key, user_key);
        assert_ne!(user_key, global_key);
        assert_ne!(session_key, global_key);
    }

    #[tokio::test]
    async fn test_metrics_accuracy() {
        let config = create_test_config();
        let plugin: RateLimitPlugin<TestRequest, TestResponse> = RateLimitPlugin::new(config);

        let session_id = SessionId::from("metrics_test");
        let user_id = (1, get_mock_address_1());

        plugin.create_session(&session_id).unwrap();

        let initial_metrics = plugin.get_metrics();
        assert_eq!(initial_metrics.total_requests, 0);
        assert_eq!(initial_metrics.rejected_requests, 0);

        // Make successful requests
        for _ in 0..3 {
            plugin.check_rate_limits(&session_id, &user_id).unwrap();
        }

        let mid_metrics = plugin.get_metrics();
        assert_eq!(mid_metrics.total_requests, 3);
        assert_eq!(mid_metrics.rejected_requests, 0);

        // Make requests that will be rejected
        for _ in 0..5 {
            let _ = plugin.check_rate_limits(&session_id, &user_id);
        }

        let final_metrics = plugin.get_metrics();
        assert_eq!(final_metrics.total_requests, 8);
        assert!(final_metrics.rejected_requests > 0);
    }
}
