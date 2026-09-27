//! Is a result the value it should be?

use serde_json::Value;

/// A result, judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Identical, floats bit for bit.
    Exact,
    /// Identical except floats within `max_ulps` of the truth.
    Inexact { floats: u64, max_ulps: u64 },
    /// A different value.
    Wrong(String),
    /// The backend refused the input.
    Rejected(String),
}

impl Verdict {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Inexact { .. } => "inexact",
            Self::Wrong(_) => "wrong",
            Self::Rejected(_) => "rejected",
        }
    }

    /// Whether the result is fit to be measured.
    pub fn measurable(&self) -> bool {
        matches!(self, Self::Exact | Self::Inexact { .. })
    }
}

/// Floats further apart than this are a different value, not a rounding.
const ROUNDING_ULPS: u64 = 4;

fn ulps(a: f64, b: f64) -> u64 {
    let key = |x: f64| {
        let bits = x.to_bits() as i64;
        // Negative floats map below zero in bit order, and -0.0 sits one
        // step below +0.0: they are different values.
        if bits < 0 { -(bits & i64::MAX) - 1 } else { bits }
    };
    key(a).abs_diff(key(b))
}

/// Compare `got` with `expected`.
pub fn compare(got: &Value, expected: &Value) -> Verdict {
    let mut floats = 0;
    let mut max_ulps = 0;
    match walk(got, expected, &mut floats, &mut max_ulps, &mut String::from("$")) {
        Err(path) => Verdict::Wrong(path),
        Ok(()) if floats == 0 => Verdict::Exact,
        Ok(()) => Verdict::Inexact { floats, max_ulps },
    }
}

fn walk(got: &Value, expected: &Value, floats: &mut u64, max: &mut u64, path: &mut String) -> Result<(), String> {
    match (got, expected) {
        (Value::Number(g), Value::Number(e)) if g.is_f64() || e.is_f64() => {
            let (g, e) = (g.as_f64().unwrap_or(f64::NAN), e.as_f64().unwrap_or(f64::NAN));
            if g.to_bits() == e.to_bits() {
                return Ok(());
            }
            let distance = ulps(g, e);
            if distance > ROUNDING_ULPS {
                return Err(format!("{path}: {g:e} != {e:e}"));
            }
            *floats += 1;
            *max = (*max).max(distance);
            Ok(())
        }
        (Value::Array(g), Value::Array(e)) if g.len() == e.len() => {
            for (i, (g, e)) in g.iter().zip(e).enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                walk(g, e, floats, max, path)?;
                path.truncate(len);
            }
            Ok(())
        }
        (Value::Object(g), Value::Object(e)) if g.len() == e.len() => {
            for (key, e) in e {
                let len = path.len();
                path.push_str(&format!(".{key}"));
                walk(
                    g.get(key).ok_or_else(|| format!("{path}: missing"))?,
                    e,
                    floats,
                    max,
                    path,
                )?;
                path.truncate(len);
            }
            Ok(())
        }
        (g, e) if g == e => Ok(()),
        _ => Err(format!("{path}: {got} != {expected}")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn floats_one_ulp_apart_are_inexact_not_wrong() {
        let a = 0.1f64;
        let b = f64::from_bits(a.to_bits() + 1);
        assert_eq!(
            compare(&json!([a]), &json!([b])),
            Verdict::Inexact { floats: 1, max_ulps: 1 }
        );
    }

    #[test]
    fn a_different_string_is_wrong() {
        assert!(matches!(
            compare(&json!({"a": "x"}), &json!({"a": "y"})),
            Verdict::Wrong(_)
        ));
    }

    #[test]
    fn a_missing_key_is_wrong() {
        assert!(matches!(compare(&json!({"a": 1}), &json!({"b": 1})), Verdict::Wrong(_)));
    }

    #[test]
    fn ulps_cross_zero() {
        assert_eq!(ulps(0.0, -0.0), 1);
        assert_eq!(ulps(f64::from_bits(1), -f64::from_bits(1)), 3);
    }
}
