use crate::obs::{client::ObsClient, requests, state::ObsStats};
use crate::server::state_store::{PolledMetrics, StateStore};

const STATS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Derives a stream bitrate from successive `GetStreamStatus.outputBytes`
/// readings, since obs-websocket does not report bitrate directly.
///
/// Each poll hands in the latest byte counter; the delta since the previous
/// poll, over the elapsed time, is the bitrate. Two situations produce no
/// sample on purpose: an inactive stream clears the baseline entirely (the
/// counter restarts from zero on the next stream, so the old baseline would
/// yield a huge negative delta), and a counter that went *backwards* while
/// active means the stream restarted between polls — that reading becomes the
/// new baseline instead of a sample.
struct BitrateTracker {
    last: Option<(u64, tokio::time::Instant)>,
}

impl BitrateTracker {
    fn new() -> Self {
        Self { last: None }
    }

    /// Feed one `GetStreamStatus` reading; returns the derived kbps when two
    /// comparable readings exist.
    fn sample(&mut self, active: bool, bytes: u64, now: tokio::time::Instant) -> Option<f64> {
        if !active {
            self.last = None;
            return None;
        }
        let kbps = self.last.and_then(|(prev_bytes, prev_time)| {
            if bytes < prev_bytes {
                return None; // stream (re)started; skip this sample
            }
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            (elapsed > 0.0).then(|| (bytes - prev_bytes) as f64 * 8.0 / 1000.0 / elapsed)
        });
        self.last = Some((bytes, now));
        kbps
    }
}

/// Poll `GetStats`/`GetStreamStatus`/`GetRecordStatus` on a fixed interval
/// and publish the results via `StateStore::update_stats`. Stream bitrate
/// isn't available directly from obs-websocket, so it's derived from the
/// delta of `GetStreamStatus.outputBytes` between polls (the same approach
/// OBS's own stats dock and most third-party remotes use).
pub(crate) fn spawn_stats_poller(client: ObsClient, state: StateStore) {
    tokio::spawn(async move {
        let mut bitrate = BitrateTracker::new();
        let mut interval = tokio::time::interval(STATS_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;

            let Ok(stats_resp) = client.request(requests::get_stats()).await else {
                // Client is most likely disconnected; the supervisor's main
                // loop will notice and drop this poller along with it.
                break;
            };
            let stats = ObsStats::from_response(&stats_resp);

            let stream_resp = client.request(requests::get_stream_status()).await.ok();
            let bitrate_kbps = stream_resp.as_ref().and_then(|v| {
                let active = v
                    .get("outputActive")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                let bytes = v.get("outputBytes").and_then(|b| b.as_u64())?;
                bitrate.sample(active, bytes, tokio::time::Instant::now())
            });
            let stream_duration_ms = stream_resp.as_ref().and_then(output_duration_if_active);

            let record_resp = client.request(requests::get_record_status()).await.ok();
            let record_duration_ms = record_resp.as_ref().and_then(output_duration_if_active);

            state
                .update_stats(PolledMetrics::new(
                    stats,
                    bitrate_kbps,
                    stream_duration_ms,
                    record_duration_ms,
                ))
                .await;
        }
    });
}

/// Extract `outputDuration` from a `GetStreamStatus`/`GetRecordStatus`
/// response, but only while `outputActive` is true (OBS keeps the last
/// duration around after stopping, which would otherwise look live).
fn output_duration_if_active(response: &serde_json::Value) -> Option<u64> {
    let active = response
        .get("outputActive")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    if !active {
        return None;
    }
    response.get("outputDuration").and_then(|d| d.as_u64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_duration_if_active_returns_none_when_inactive() {
        let v = serde_json::json!({ "outputActive": false, "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), None);
    }

    #[test]
    fn output_duration_if_active_returns_duration_when_active() {
        let v = serde_json::json!({ "outputActive": true, "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), Some(5000));
    }

    #[test]
    fn output_duration_if_active_defaults_missing_active_to_false() {
        let v = serde_json::json!({ "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), None);
    }

    #[test]
    fn bitrate_tracker_derives_kbps_from_the_byte_delta() {
        let mut tracker = BitrateTracker::new();
        let start = tokio::time::Instant::now();

        // The first reading is a baseline: nothing to compare against yet.
        assert_eq!(tracker.sample(true, 1_000, start), None);

        // 250 000 bytes over 2 s = 2 000 000 bits / 2 s = 1000 kbps.
        let kbps = tracker
            .sample(true, 251_000, start + std::time::Duration::from_secs(2))
            .expect("two active readings must produce a sample");
        assert!((kbps - 1000.0).abs() < f64::EPSILON, "got {kbps}");
    }

    #[test]
    fn bitrate_tracker_skips_the_sample_when_the_counter_goes_backwards() {
        let mut tracker = BitrateTracker::new();
        let start = tokio::time::Instant::now();

        assert_eq!(tracker.sample(true, 500_000, start), None);
        // The counter restarted (stream restart): no sample, but the reading
        // becomes the new baseline...
        let later = start + std::time::Duration::from_secs(2);
        assert_eq!(tracker.sample(true, 1_000, later), None);
        // ...so the next delta is measured from it.
        let kbps = tracker
            .sample(true, 251_000, later + std::time::Duration::from_secs(2))
            .expect("the restart reading must have become the baseline");
        assert!((kbps - 1000.0).abs() < f64::EPSILON, "got {kbps}");
    }

    #[test]
    fn bitrate_tracker_resets_its_baseline_while_inactive() {
        let mut tracker = BitrateTracker::new();
        let start = tokio::time::Instant::now();

        assert_eq!(tracker.sample(true, 500_000, start), None);
        // An inactive poll clears the baseline entirely.
        let later = start + std::time::Duration::from_secs(2);
        assert_eq!(tracker.sample(false, 500_000, later), None);
        // The next active reading starts over rather than comparing against
        // the pre-stop counter.
        let resumed = later + std::time::Duration::from_secs(2);
        assert_eq!(tracker.sample(true, 900_000, resumed), None);
    }
}
