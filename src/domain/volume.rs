//! Conversions between the two ways this project talks about audio level.
//!
//! OBS works in a *linear multiplier*: `0.0` is silence, `1.0` is unity gain
//! (the input plays back at the volume it arrived at), and values above `1.0`
//! amplify. Users, and the CLI/TUI surfaces they see, work in a *percent* from
//! `0` to `100`.
//!
//! The two are not proportional. Human loudness perception is closer to a
//! square curve, so a slider at 50% should sound like "half as loud", not carry
//! half the signal energy. `percent_to_mul` therefore squares the fraction
//! (`0.5 * 0.5 = 0.25`) and `mul_to_percent` takes the square root back, which
//! is why the two functions are exact inverses only up to rounding.

/// Percent (`0..=100`) to the linear multiplier OBS expects.
///
/// The `.powi(2)` is the perceptual curve described in the module docs: 50%
/// becomes `0.25`, not `0.5`.
///
/// Values above `100` are *not* rejected — `u8` allows up to `255`, and such a
/// value simply produces a multiplier above `1.0`, i.e. amplification. Callers
/// that want an upper bound must impose it themselves.
pub fn percent_to_mul(percent: u8) -> f64 {
    (percent as f64 / 100.0).powi(2)
}

/// Linear multiplier back to a whole percent, for display.
///
/// The clamp runs *after* rounding, so any input that cannot produce a sensible
/// percent collapses to `0`: `sqrt` of a negative multiplier is NaN, and casting
/// a NaN or a below-range value to `u8` after `clamp(0.0, 100.0)` yields `0`.
/// A multiplier above `1.0` (amplification) is reported as `100`.
pub fn mul_to_percent(mul: f64) -> u8 {
    (mul.sqrt() * 100.0).round().clamp(0.0, 100.0) as u8
}

/// Linear multiplier to decibels, the unit OBS shows next to its faders.
///
/// Silence has no finite decibel value, so a zero — or negative — multiplier
/// maps to [`f64::NEG_INFINITY`] rather than returning an error. Callers render
/// that as "-inf dB" or as a muted fader; there is nothing for them to recover
/// from.
pub fn mul_to_db(mul: f64) -> f64 {
    if mul <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * mul.log10()
    }
}

/// Whether `value` is usable as a linear volume multiplier.
///
/// A multiplier must be finite (NaN and the infinities have no meaning on a
/// fader) and at or above zero, because a negative multiplier would invert the
/// waveform — flipping its phase — rather than making it quieter.
///
/// Callers decide what a rejection means: building an OBS request turns it into
/// an error, while decoding an inbound OBS event drops the event.
pub fn is_valid_multiplier(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_roundtrip() {
        for p in [0u8, 50, 100] {
            assert_eq!(mul_to_percent(percent_to_mul(p)), p);
        }
    }

    #[test]
    fn zero_mul_is_neg_infinity() {
        assert!(mul_to_db(0.0).is_infinite());
    }

    #[test]
    fn multiplier_validity_rejects_negative_and_non_finite() {
        assert!(is_valid_multiplier(0.0));
        assert!(is_valid_multiplier(1.5));
        assert!(!is_valid_multiplier(-0.1));
        assert!(!is_valid_multiplier(f64::NAN));
        assert!(!is_valid_multiplier(f64::INFINITY));
    }
}
