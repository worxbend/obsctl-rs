use std::time::Duration;

use rand::Rng as _;

use crate::config::model::ReconnectConfig;

#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    config: ReconnectConfig,
    attempts: u32,
    /// Whether the delay just handed out was the longest this policy will ever
    /// hand out. Only a bounded policy (`endless: false`) acts on it, and it is
    /// what makes the attempt that waits the full `max_delay_ms` the last one
    /// rather than the first one refused.
    backoff_topped_out: bool,
}

impl ReconnectPolicy {
    pub fn new(config: ReconnectConfig) -> Self {
        Self {
            config,
            attempts: 0,
            backoff_topped_out: false,
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// How long to wait before the next connection attempt, or `None` to stop
    /// trying.
    ///
    /// Each attempt waits longer than the last — `initial_delay_ms` multiplied
    /// by `multiplier` once per attempt so far — up to a ceiling of
    /// `max_delay_ms`, plus a random slice of `jitter_ms` so that several
    /// daemons restarted together do not all redial on the same tick.
    ///
    /// Two settings can end the retrying:
    ///
    /// - `enabled: false` means never retry, so the very first call is `None`.
    /// - `endless: false` means retry, but not forever. The stopping rule is
    ///   "give up once backing off has stopped helping": the delay grows to the
    ///   `max_delay_ms` ceiling, that longest wait is used once, and the call
    ///   after it returns `None`. With the shipped defaults (500 ms, ×1.8,
    ///   ceiling 10 s) that is seven attempts spread over about half a minute.
    ///
    /// That rule reuses `max_delay_ms` rather than introducing a separate
    /// "maximum attempts" setting, so the number of retries follows the same
    /// knob a user already turns to say how patient the daemon should be, and
    /// no new config key has to be documented and validated. `endless: true` —
    /// the default, and what every earlier version did — never stops.
    pub fn next_delay(&mut self) -> Option<Duration> {
        if !self.config.enabled {
            return None;
        }

        if !self.config.endless && self.backoff_topped_out {
            return None;
        }

        let base =
            self.config.initial_delay_ms as f64 * self.config.multiplier.powi(self.attempts as i32);
        let ceiling = self.config.max_delay_ms as f64;
        let capped = base.min(ceiling);

        // A `multiplier` of exactly 1.0 is allowed by the config schema and
        // makes the delay flat, so it would never climb to the ceiling on its
        // own and a bounded policy would quietly become an unbounded one. A
        // backoff that does not back off has stopped helping from the outset.
        self.backoff_topped_out = capped >= ceiling || self.config.multiplier <= 1.0;

        let jitter = rand::thread_rng().gen_range(0..=self.config.jitter_ms) as f64;
        let ms = (capped + jitter) as u64;
        self.attempts = self.attempts.saturating_add(1);
        Some(Duration::from_millis(ms))
    }

    /// Start the backoff over, after a connection succeeded.
    pub fn reset(&mut self) {
        self.attempts = 0;
        self.backoff_topped_out = false;
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

    /// Ask a policy for delays until it gives up, and report how many it
    /// handed out. Bounded by a generous ceiling so that a policy that should
    /// stop but does not fails the test instead of hanging it.
    fn attempts_until_giving_up(policy: &mut ReconnectPolicy) -> Option<usize> {
        const RUNAWAY: usize = 1_000;
        (0..RUNAWAY).find(|_| policy.next_delay().is_none())
    }

    fn bounded(config: ReconnectConfig) -> ReconnectPolicy {
        ReconnectPolicy::new(ReconnectConfig {
            endless: false,
            ..config
        })
    }

    /// `endless: true` is the documented default and what every earlier version
    /// did: the daemon keeps trying for as long as it is running.
    #[test]
    fn endless_policy_never_gives_up() {
        let mut p = policy();
        assert!(p.enabled());
        assert_eq!(
            attempts_until_giving_up(&mut p),
            None,
            "the default policy must keep offering delays"
        );
    }

    /// `endless: false` stops once the backoff has grown to `max_delay_ms`,
    /// because every attempt after that would wait exactly as long as the last
    /// one. With the shipped defaults that is a handful of attempts, not one
    /// and not forever.
    #[test]
    fn bounded_policy_stops_once_the_delay_reaches_the_ceiling() {
        let mut p = bounded(ReconnectConfig::default());
        let granted = attempts_until_giving_up(&mut p).expect("a bounded policy must give up");
        assert!(
            (2..20).contains(&granted),
            "expected a handful of attempts, got {granted}"
        );
    }

    /// The last delay a bounded policy hands out is at the ceiling, so it does
    /// not stop while there is still cheap, quick retrying left to do.
    #[test]
    fn bounded_policy_uses_its_full_backoff_before_giving_up() {
        let config = ReconnectConfig {
            jitter_ms: 0,
            ..ReconnectConfig::default()
        };
        let mut p = bounded(config.clone());

        let mut last = Duration::from_millis(0);
        while let Some(delay) = p.next_delay() {
            last = delay;
        }
        assert_eq!(last, Duration::from_millis(config.max_delay_ms));
    }

    /// A config where the first delay is already at the ceiling still gets one
    /// retry. `endless: false` means "not forever"; refusing to try at all is
    /// what `enabled: false` says.
    #[test]
    fn bounded_policy_grants_one_retry_when_it_starts_at_the_ceiling() {
        let mut p = bounded(ReconnectConfig {
            initial_delay_ms: 10_000,
            max_delay_ms: 10_000,
            ..ReconnectConfig::default()
        });
        assert_eq!(attempts_until_giving_up(&mut p), Some(1));
    }

    /// `multiplier: 1.0` passes config validation and makes the delay flat, so
    /// it would never climb to the ceiling. Without the flat-backoff case the
    /// bound would never fire and `endless: false` would silently retry
    /// forever.
    #[test]
    fn bounded_policy_stops_even_when_the_backoff_is_flat() {
        let mut p = bounded(ReconnectConfig {
            multiplier: 1.0,
            ..ReconnectConfig::default()
        });
        assert_eq!(attempts_until_giving_up(&mut p), Some(1));
    }

    /// A successful connect resets the count, so a daemon that reconnects, runs
    /// for a week and then loses OBS again gets the full budget a second time
    /// rather than the one attempt left over from last time.
    #[test]
    fn reset_restores_a_bounded_policy_budget() {
        let mut p = bounded(ReconnectConfig::default());
        let first = attempts_until_giving_up(&mut p).expect("a bounded policy must give up");
        p.reset();
        let second = attempts_until_giving_up(&mut p).expect("a bounded policy must give up");
        assert_eq!(first, second);
    }

    /// `enabled: false` still wins over `endless`: no attempt at all.
    #[test]
    fn disabled_policy_returns_none_even_when_endless() {
        let mut p = ReconnectPolicy::new(ReconnectConfig {
            enabled: false,
            endless: true,
            ..ReconnectConfig::default()
        });
        assert!(p.next_delay().is_none());
    }
}
