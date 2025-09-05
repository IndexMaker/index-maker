use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use chrono::{DateTime, Duration, Utc};
use itertools::{
    FoldWhile::{Continue, Done},
    Itertools,
};

pub struct LimitWeight {
    weight: usize,
    timestamp: DateTime<Utc>,
}

pub struct LimitWeights {
    weights: VecDeque<LimitWeight>,
}

impl LimitWeights {
    pub fn new() -> Self {
        Self {
            weights: VecDeque::new(),
        }
    }

    pub fn current_weight(&self, period: Duration, timestamp: DateTime<Utc>) -> usize {
        self.weights
            .iter()
            .take_while(|x| timestamp - x.timestamp < period)
            .map(|x| x.weight)
            .sum()
    }

    pub fn waiting_period(
        &self,
        release_weight: usize,
        period: Duration,
        timestamp: DateTime<Utc>,
    ) -> Duration {
        // Tell the time of the oldest event in the window
        let boundary_time = timestamp - period;

        // Collect a window of events
        let window = self
            .weights
            .iter()
            .take_while(|x| boundary_time < x.timestamp)
            .collect_vec();

        // Walk from the oldest event, and find how many events need to drop off
        // the window in order to release weight.
        let outcome = window
            .into_iter()
            .rev()
            .fold_while((timestamp, 0usize), |(t, s), w| {
                if s < release_weight {
                    Continue((w.timestamp, s + w.weight))
                } else {
                    Done((t, s))
                }
            });

        // If we managed to collect enough weight from events dropped off the
        // window still keeping some events in the window, then we can wait
        // the amount of time required to drop those events, otherwise we
        // need to wait whole period less time since most recent event.
        match outcome {
            Done((t, _)) => t - boundary_time,
            Continue((t, _)) => period - (timestamp - t),
        }
    }

    pub fn put_weight(&mut self, weight: usize, timestamp: DateTime<Utc>) {
        self.weights.push_front(LimitWeight { weight, timestamp });
    }

    pub fn purge(&mut self, period: Duration, timestamp: DateTime<Utc>) {
        let pos = self
            .weights
            .iter()
            .find_position(|x| period < timestamp - x.timestamp)
            .map(|(p, _)| p);

        if let Some(pos) = pos {
            self.weights.drain(pos..);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimiterConfig {
    pub limit: usize,
    pub period: Duration,
}

impl LimiterConfig {
    pub fn new(limit: usize, period: Duration) -> Self {
        Self { limit, period }
    }
}

pub struct Limiter {
    weights: LimitWeights,
    config: LimiterConfig,
}

impl Limiter {
    pub fn new(limit: usize, period: Duration) -> Self {
        Self {
            weights: LimitWeights::new(),
            config: LimiterConfig::new(limit, period),
        }
    }

    pub fn refit(&mut self, config: LimiterConfig, count: usize, timestamp: DateTime<Utc>) {
        let current_weight = self.weights.current_weight(config.period, timestamp);
        if current_weight < count {
            let extra_weight = count - current_weight;
            self.weights.put_weight(extra_weight, timestamp);
        }
        self.config = config;
    }

    pub fn try_consume(&mut self, weight: usize, timestamp: DateTime<Utc>) -> bool {
        if self.weights.current_weight(self.config.period, timestamp) + weight <= self.config.limit
        {
            self.weights.put_weight(weight, timestamp);
            true
        } else {
            self.weights.purge(self.config.period, timestamp);
            false
        }
    }

    pub fn waiting_period(&self, release_weight: usize, timestamp: DateTime<Utc>) -> Duration {
        self.weights
            .waiting_period(release_weight, self.config.period, timestamp)
    }

    pub fn waiting_period_half_limit(&self, timestamp: DateTime<Utc>) -> Duration {
        self.weights
            .waiting_period(self.config.limit / 2, self.config.period, timestamp)
    }
}

pub struct MultiLimiter {
    weights: LimitWeights,
    configs: Vec<LimiterConfig>,
}

impl MultiLimiter {
    pub fn new(configs: Vec<LimiterConfig>) -> Self {
        Self {
            weights: LimitWeights::new(),
            configs,
        }
    }

    pub fn refit(&mut self, values: Vec<(LimiterConfig, usize)>, timestamp: DateTime<Utc>) {
        self.configs.clear();
        for (config, count) in values {
            let current_weight = self.weights.current_weight(config.period, timestamp);
            if current_weight < count {
                let extra_weight = count - current_weight;
                self.weights.put_weight(extra_weight, timestamp);
            }
            self.configs.push(config);
        }
    }

    pub fn check_limit(
        &self,
        weight: usize,
        config: &LimiterConfig,
        timestamp: DateTime<Utc>,
    ) -> bool {
        self.weights.current_weight(config.period, timestamp) + weight <= config.limit
    }

    pub fn try_consume(&mut self, weight: usize, timestamp: DateTime<Utc>) -> bool {
        if self
            .configs
            .iter()
            .all(|config| self.check_limit(weight, config, timestamp))
        {
            self.weights.put_weight(weight, timestamp);
            true
        } else {
            if let Some(purge_period) = self.configs.iter().map(|config| config.period).max() {
                self.weights.purge(purge_period, timestamp);
            }
            false
        }
    }

    pub fn waiting_period(&self, release_weight: usize, timestamp: DateTime<Utc>) -> Duration {
        self.configs
            .iter()
            .filter_map(|config| {
                if !self.check_limit(release_weight, config, timestamp) {
                    Some(
                        self.weights
                            .waiting_period(release_weight, config.period, timestamp),
                    )
                } else {
                    None
                }
            })
            .max()
            .unwrap_or_default()
    }

    pub fn waiting_period_half_smallest_limit(&self, timestamp: DateTime<Utc>) -> Duration {
        let release_weight = (self
            .configs
            .iter()
            .map(|config| config.limit)
            .min()
            .unwrap_or_default()
            / 2)
        .max(1usize);

        self.configs
            .iter()
            .filter_map(|config| {
                if !self.check_limit(release_weight, config, timestamp) {
                    Some(
                        self.weights
                            .waiting_period(release_weight, config.period, timestamp),
                    )
                } else {
                    None
                }
            })
            .max()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod test {
    use chrono::{Duration, TimeDelta, Utc};

    use crate::core::limit::LimitWeights;

    #[test]
    fn test_limit_weights() {
        let mut timestamp = Utc::now();
        let mut limiter = LimitWeights::new();

        assert_eq!(limiter.current_weight(Duration::seconds(10), timestamp), 0);

        // Here in this test we control the time explicilty stating how much passes.

        timestamp += TimeDelta::seconds(3);

        // Put some event with some weight

        limiter.put_weight(5, timestamp);

        assert_eq!(limiter.current_weight(Duration::seconds(10), timestamp), 5);

        timestamp += TimeDelta::seconds(3);

        // Put another event with other weight

        limiter.put_weight(3, timestamp);

        timestamp += TimeDelta::seconds(3);

        // Note that:
        // - 3 seconds ago we put 3, and
        // - 6 seconds ago we put 5

        // Here we're asking:
        // - How much altogether weight is currently in a window of given length?

        assert_eq!(limiter.current_weight(Duration::seconds(10), timestamp), 8);
        assert_eq!(limiter.current_weight(Duration::seconds(5), timestamp), 3);
        assert_eq!(limiter.current_weight(Duration::seconds(1), timestamp), 0);

        // Here we're asking:
        // - How much time I need to wait for rolling window of given length
        //   to contain in it altogether less than the given weight?

        // Note that as we're waiting the window is moving forwards, and events
        // are dropping off from it. We're asking how much this window needs to
        // move forward so that enough events drops off, so that their total
        // weight will be no less than given weight.

        assert_eq!(
            limiter.waiting_period(1, Duration::seconds(20), timestamp),
            Duration::seconds(14)
        );
        assert_eq!(
            limiter.waiting_period(5, Duration::seconds(20), timestamp),
            Duration::seconds(14)
        );
        assert_eq!(
            limiter.waiting_period(6, Duration::seconds(20), timestamp),
            Duration::seconds(17)
        );
        assert_eq!(
            limiter.waiting_period(9, Duration::seconds(20), timestamp),
            Duration::seconds(17)
        );

        assert_eq!(
            limiter.waiting_period(1, Duration::seconds(5), timestamp),
            Duration::seconds(2)
        );
        assert_eq!(
            limiter.waiting_period(3, Duration::seconds(5), timestamp),
            Duration::seconds(2)
        );
        assert_eq!(
            limiter.waiting_period(9, Duration::seconds(5), timestamp),
            Duration::seconds(2)
        );

        timestamp += TimeDelta::seconds(3);

        // After time passed some events dropped off

        assert_eq!(limiter.current_weight(Duration::seconds(10), timestamp), 8);
        assert_eq!(limiter.current_weight(Duration::seconds(5), timestamp), 0);

        timestamp += TimeDelta::seconds(3);

        assert_eq!(limiter.current_weight(Duration::seconds(10), timestamp), 3);
        assert_eq!(limiter.current_weight(Duration::seconds(5), timestamp), 0);

        timestamp += TimeDelta::seconds(3);

        // Eventually all events drop off

        assert_eq!(limiter.current_weight(Duration::seconds(10), timestamp), 0);
    }
}
