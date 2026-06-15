use std::time::Duration;

use rand::Rng as _;

use crate::config::model::ReconnectConfig;

#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    config: ReconnectConfig,
    attempts: u32,
}

impl ReconnectPolicy {
    pub fn new(config: ReconnectConfig) -> Self {
        Self {
            config,
            attempts: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn next_delay(&mut self) -> Option<Duration> {
        if !self.config.enabled {
            return None;
        }

        let base =
            self.config.initial_delay_ms as f64 * self.config.multiplier.powi(self.attempts as i32);
        let capped = base.min(self.config.max_delay_ms as f64);
        let jitter = rand::thread_rng().gen_range(0..=self.config.jitter_ms) as f64;
        let ms = (capped + jitter) as u64;
        self.attempts = self.attempts.saturating_add(1);
        Some(Duration::from_millis(ms))
    }

    pub fn reset(&mut self) {
        self.attempts = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::ReconnectConfig;

    fn policy() -> ReconnectPolicy {
        ReconnectPolicy::new(ReconnectConfig::default())
    }

    #[test]
    fn disabled_policy_returns_none() {
        let mut p = ReconnectPolicy::new(ReconnectConfig {
            enabled: false,
            ..ReconnectConfig::default()
        });
        assert!(p.next_delay().is_none());
    }

    #[test]
    fn delay_grows_over_time() {
        let mut p = policy();
        let d1 = p.next_delay().unwrap();
        let d2 = p.next_delay().unwrap();
        assert!(d2 >= d1);
    }

    #[test]
    fn reset_reduces_delay() {
        let mut p = policy();
        for _ in 0..5 {
            p.next_delay();
        }
        let after_many = p.next_delay().unwrap();
        p.reset();
        let after_reset = p.next_delay().unwrap();
        assert!(after_reset <= after_many);
    }
}
