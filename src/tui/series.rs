/// A bounded rolling window of metric samples, used by the sparklines and the
/// braille meters.
///
/// Every metric the TUI graphs wants the same three things — append a sample,
/// forget the oldest once the window is full, and know the highest value seen
/// so far — so they are defined once here rather than in each widget. `peak`
/// takes the live value because a fresh spike above every stored sample must
/// still win: the meter's scale may never trail the mark it is drawing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RollingSeries {
    samples: Vec<f64>,
}

impl RollingSeries {
    /// How many samples a series keeps. At the two-second stats poll interval
    /// this is a little over a minute of history, which is what the widest
    /// sparkline can draw.
    pub const CAPACITY: usize = 32;

    pub fn push(&mut self, value: f64) {
        self.samples.push(value);
        if self.samples.len() > Self::CAPACITY {
            self.samples.drain(0..self.samples.len() - Self::CAPACITY);
        }
    }

    pub fn samples(&self) -> &[f64] {
        &self.samples
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The highest value across the stored samples and `current`.
    pub fn peak(&self, current: f64) -> f64 {
        self.samples.iter().copied().fold(current, f64::max)
    }
}

impl FromIterator<f64> for RollingSeries {
    fn from_iter<I: IntoIterator<Item = f64>>(iter: I) -> Self {
        let mut series = Self::default();
        for value in iter {
            series.push(value);
        }
        series
    }
}

#[cfg(test)]
mod tests {
    use super::RollingSeries;

    #[test]
    fn push_drops_the_oldest_samples_past_capacity() {
        let series: RollingSeries = (0..40).map(f64::from).collect();
        assert_eq!(series.samples().len(), RollingSeries::CAPACITY);
        assert_eq!(series.samples().first(), Some(&8.0));
        assert_eq!(series.samples().last(), Some(&39.0));
    }

    #[test]
    fn peak_tracks_the_session_high_but_never_trails_the_live_value() {
        let series: RollingSeries = [10.0, 40.0, 20.0].into_iter().collect();
        assert_eq!(series.peak(20.0), 40.0);
        // A fresh spike above every recorded sample wins.
        assert_eq!(series.peak(90.0), 90.0);
        // No history at all falls back to the live value.
        assert_eq!(RollingSeries::default().peak(12.0), 12.0);
    }
}
