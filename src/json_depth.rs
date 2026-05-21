//! JSON depth validation for attacker-controlled `extra` fields.
//!
//! The 64 KiB request body limit (`RequestBodyLimitLayer`) bounds total
//! payload size, but inside that budget an attacker can still craft deeply
//! nested JSON that wastes CPU during downstream handling and pushes
//! `serde_json`'s default 128-level recursion close to its limit. The
//! `extra` field on `PaymentRequirements` and related types is the most
//! attacker-controlled JSON we expose, since clients can place arbitrary
//! application-specific data in it.
//!
//! This module provides a `serde` `deserialize_with` shim that rejects
//! `extra` values whose maximum nesting depth exceeds [`MAX_EXTRA_JSON_DEPTH`].
//! Legitimate `extra` payloads observed in production are shallow (typically
//! `{name, version}`, depth 2), so the cap is generous enough not to break
//! any known integration.

use serde::de::{Deserializer, Error};
use serde::Deserialize;

/// Maximum allowed JSON nesting depth for `extra` payloads.
///
/// 16 is comfortably above any observed legitimate depth (2-3) and well below
/// `serde_json`'s 128-level default recursion limit. The depth is counted by
/// containers only: leaf values (strings, numbers, bools, null) do not add to
/// the depth.
pub const MAX_EXTRA_JSON_DEPTH: usize = 16;

/// Compute the maximum container-nesting depth of a `serde_json::Value`
/// iteratively, with no recursion of its own. Returns 0 for scalar leaves.
fn json_value_depth(root: &serde_json::Value) -> usize {
    let mut max_depth = 0usize;
    let mut stack: Vec<(&serde_json::Value, usize)> = vec![(root, 0)];
    while let Some((v, d)) = stack.pop() {
        if d > max_depth {
            max_depth = d;
        }
        match v {
            serde_json::Value::Array(arr) => {
                for x in arr.iter() {
                    stack.push((x, d + 1));
                }
            }
            serde_json::Value::Object(obj) => {
                for x in obj.values() {
                    stack.push((x, d + 1));
                }
            }
            _ => {}
        }
    }
    max_depth
}

/// `serde` `deserialize_with` shim for `Option<serde_json::Value>` fields.
///
/// Use on any `extra: Option<serde_json::Value>` that is parsed from external
/// input:
///
/// ```ignore
/// #[serde(default, deserialize_with = "crate::json_depth::deserialize_bounded_extra")]
/// pub extra: Option<serde_json::Value>,
/// ```
pub fn deserialize_bounded_extra<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(deserializer)?;
    if let Some(ref val) = v {
        let depth = json_value_depth(val);
        if depth > MAX_EXTRA_JSON_DEPTH {
            return Err(D::Error::custom(format!(
                "extra field nesting depth {} exceeds maximum of {}",
                depth, MAX_EXTRA_JSON_DEPTH
            )));
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_value_has_depth_zero() {
        let v: serde_json::Value = serde_json::from_str(r#""hello""#).unwrap();
        assert_eq!(json_value_depth(&v), 0);
    }

    #[test]
    fn single_object_has_depth_one() {
        let v: serde_json::Value = serde_json::from_str(r#"{"name": "USDC"}"#).unwrap();
        assert_eq!(json_value_depth(&v), 1);
    }

    #[test]
    fn nested_object_counts_each_level() {
        let v: serde_json::Value = serde_json::from_str(r#"{"a": {"b": {"c": 1}}}"#).unwrap();
        assert_eq!(json_value_depth(&v), 3);
    }

    #[test]
    fn array_of_arrays_counts_each_level() {
        let v: serde_json::Value = serde_json::from_str(r#"[[[[[]]]]]"#).unwrap();
        assert_eq!(json_value_depth(&v), 5);
    }

    #[test]
    fn deep_payload_rejected_by_deserializer() {
        // 20 nested objects, above the 16-level cap.
        let deep = "{\"a\":".repeat(20) + "1" + &"}".repeat(20);
        let wrapped = format!("{{\"extra\": {}}}", deep);

        #[derive(serde::Deserialize)]
        struct T {
            #[serde(default, deserialize_with = "deserialize_bounded_extra")]
            #[allow(dead_code)]
            extra: Option<serde_json::Value>,
        }

        let result: Result<T, _> = serde_json::from_str(&wrapped);
        assert!(result.is_err(), "20-level nesting should be rejected");
    }

    #[test]
    fn shallow_legit_payload_accepted() {
        #[derive(serde::Deserialize)]
        struct T {
            #[serde(default, deserialize_with = "deserialize_bounded_extra")]
            extra: Option<serde_json::Value>,
        }

        let payload = r#"{"extra": {"name": "USDC", "version": "2"}}"#;
        let parsed: T = serde_json::from_str(payload).expect("should parse legit extra");
        assert!(parsed.extra.is_some());
    }
}
