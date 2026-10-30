//! Read JSON Schema integers without rewriting immutable observation JSON.
use serde_json::Value;

/// Integer JSON encodings retain their full u64 precision. Floating encodings
/// must be whole, nonnegative and within JSON's interoperable exact range.
/// Historical values outside that range stay unavailable rather than rounding.
pub(crate) fn unsigned_integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|number| {
                number.is_finite()
                    && (0.0..=9_007_199_254_740_991.0).contains(number)
                    && number.fract() == 0.0
            })
            .map(|number| number as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn schema_integer_representations_are_exact_and_number_fields_are_untouched() {
        for text in ["20", "20.0", "2e1", "-0.0"] {
            assert_eq!(
                unsigned_integer(&serde_json::from_str::<Value>(text).unwrap()),
                Some(if text == "-0.0" { 0 } else { 20 })
            );
        }
        assert_eq!(unsigned_integer(&json!(u64::MAX)), Some(u64::MAX));
        for value in [
            json!(20.5),
            json!(-1),
            json!(9007199254740992.0),
            json!("20"),
        ] {
            assert_eq!(unsigned_integer(&value), None);
        }
    }
}
