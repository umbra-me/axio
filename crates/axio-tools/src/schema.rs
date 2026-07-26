//! Building tool schemas, and reading arguments back out.
//!
//! Schemas are built through these helpers rather than by hand so that the JSON
//! is byte-identical every run. Prompt caching is a prefix match and tool
//! definitions render first, so an unstable schema costs the entire cached
//! prefix with nothing to show for it and no error anywhere.

use axio_core::tool::ToolError;
use serde_json::{Map, Value, json};

pub fn string(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

pub fn integer(description: &str) -> Value {
    json!({ "type": "integer", "description": description })
}

pub fn boolean(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

/// Insertion order is preserved (`serde_json/preserve_order`), so the field
/// order here is the field order on the wire, every time.
pub fn object(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let mut props = Map::new();
    for (name, spec) in properties {
        props.insert((*name).to_owned(), spec.clone());
    }
    let mut root = Map::new();
    root.insert("type".into(), json!("object"));
    root.insert("properties".into(), Value::Object(props));
    root.insert("required".into(), json!(required));
    root.insert("additionalProperties".into(), json!(false));
    Value::Object(root)
}

pub fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::BadInput(format!("`{key}` is required and must be a string")))
}

pub fn opt_str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub fn usize_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

pub fn bool_arg(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_order_is_stable_across_builds() {
        let build = || {
            object(
                &[
                    ("path", string("a path")),
                    ("offset", integer("an offset")),
                    ("limit", integer("a limit")),
                ],
                &["path"],
            )
        };
        let first = serde_json::to_string(&build()).unwrap();
        for _ in 0..100 {
            assert_eq!(serde_json::to_string(&build()).unwrap(), first);
        }
        // And the order is the one written, not alphabetical.
        assert!(first.find("\"path\"").unwrap() < first.find("\"offset\"").unwrap());
    }

    #[test]
    fn a_missing_argument_is_reported_by_name() {
        let err = str_arg(&json!({}), "path").unwrap_err();
        assert!(err.to_string().contains("path"));
    }

    #[test]
    fn a_wrong_type_is_a_bad_input_not_a_silent_default() {
        assert!(str_arg(&json!({"path": 7}), "path").is_err());
        assert_eq!(usize_arg(&json!({"offset": "x"}), "offset"), None);
    }
}
