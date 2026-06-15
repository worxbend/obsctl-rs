pub fn percent_to_mul(percent: u8) -> f64 {
    (percent as f64 / 100.0).powi(2)
}

pub fn mul_to_percent(mul: f64) -> u8 {
    (mul.sqrt() * 100.0).round().clamp(0.0, 100.0) as u8
}

pub fn mul_to_db(mul: f64) -> f64 {
    if mul <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * mul.log10()
    }
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
}
