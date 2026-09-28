//! Explicit duration operands shared by all public CLI timeout flags.

const MAX_MILLIS: u64 = 24 * 60 * 60 * 1000;

/// Parse an integer duration with an explicit unit. Never guess milliseconds.
pub fn milliseconds(raw: &str) -> Result<u64, String> {
    let boundary = raw
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(raw.len());
    let (number, unit) = raw.split_at(boundary);
    let multiplier = match unit {
        "ms" => 1,
        "s" => 1000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => {
            return Err(
                "duration requires an explicit unit: ms, s, m, or h (for example 500ms or 30s)"
                    .into(),
            );
        }
    };
    let value = number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .filter(|value| (1..=MAX_MILLIS).contains(value))
        .ok_or_else(|| "duration must be a positive integer between 1ms and 24h".to_owned())?;
    Ok(value)
}

/// Adapt to existing APIs whose contract is integral seconds, without rounding.
pub fn seconds(raw: &str) -> Result<u64, String> {
    let millis = milliseconds(raw)?;
    if !millis.is_multiple_of(1000) {
        return Err("this operation requires whole seconds; use 1s rather than 500ms".into());
    }
    Ok(millis / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_units_are_converted_exactly() {
        for (raw, expected) in [
            ("1ms", 1),
            ("500ms", 500),
            ("30s", 30_000),
            ("2m", 120_000),
            ("24h", 86_400_000),
        ] {
            assert_eq!(milliseconds(raw).unwrap(), expected);
        }
        assert_eq!(seconds("120s").unwrap(), 120);
        assert_eq!(seconds("2000ms").unwrap(), 2);
        assert!(
            seconds("1500ms").is_err(),
            "fractional seconds must not be silently rounded"
        );
    }
    #[test]
    fn ambiguity_overflow_and_unbounded_waits_are_rejected() {
        for raw in [
            "",
            "30",
            "ms",
            "0s",
            "-1s",
            "1.5s",
            "25h",
            "86400001ms",
            "18446744073709551615h",
            "1d",
            "1s\n",
            " 1s",
        ] {
            assert!(
                milliseconds(raw).is_err(),
                "unexpected duration accepted: {raw:?}"
            );
        }
    }
}
