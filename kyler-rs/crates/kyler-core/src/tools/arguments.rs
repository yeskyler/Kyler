//! Validate advertised argument structure before permission review or execution.
use anyhow::{Result, ensure};
use serde_json::Value;

pub(crate) fn validate(schema: &Value, value: &Value) -> Result<()> {
    check(schema, schema, value, "$", 0)
}
fn check(root: &Value, schema: &Value, value: &Value, path: &str, depth: usize) -> Result<()> {
    ensure!(depth <= 64, "tool arguments exceed schema nesting limit");
    if let Some(reference) = schema["$ref"]
        .as_str()
        .and_then(|reference| reference.strip_prefix('#'))
    {
        if let Some(target) = root.pointer(reference) {
            return check(root, target, value, path, depth + 1);
        }
    }
    if let Some(variants) = schema["anyOf"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
    {
        ensure!(
            variants
                .iter()
                .any(|schema| check(root, schema, value, path, depth + 1).is_ok()),
            "invalid tool argument at {path}: no matching schema variant"
        );
    }
    let matches = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.as_f64().is_some_and(|n| n.fract() == 0.0),
        _ => true,
    };
    if let Some(kind) = schema["type"].as_str() {
        ensure!(
            matches(kind),
            "invalid tool argument at {path}: expected {kind}"
        );
    }
    if let Some(kinds) = schema["type"].as_array() {
        ensure!(
            kinds.iter().filter_map(Value::as_str).any(matches),
            "invalid tool argument type at {path}"
        );
    }
    if let Some(values) = schema["enum"].as_array() {
        ensure!(
            values.contains(value),
            "invalid tool argument at {path}: unsupported value"
        );
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema["required"].as_array() {
            for key in required.iter().filter_map(Value::as_str) {
                ensure!(
                    object.contains_key(key),
                    "missing required tool argument: {path}.{key}"
                );
            }
        }
        for (key, value) in object {
            if let Some(property) = schema["properties"].get(key) {
                check(root, property, value, &format!("{path}.{key}"), depth + 1)?;
            } else if schema["additionalProperties"] == false {
                anyhow::bail!("unknown tool argument: {path}.{key}");
            } else if schema["additionalProperties"].is_object() {
                check(
                    root,
                    &schema["additionalProperties"],
                    value,
                    &format!("{path}.{key}"),
                    depth + 1,
                )?;
            }
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        for (index, value) in values.iter().enumerate() {
            check(root, items, value, &format!("{path}[{index}]"), depth + 1)?;
        }
    }
    if let Some(number) = value.as_f64() {
        if let Some(min) = schema["minimum"].as_f64() {
            ensure!(number >= min, "tool argument below minimum at {path}");
        }
        if let Some(max) = schema["maximum"].as_f64() {
            ensure!(number <= max, "tool argument above maximum at {path}");
        }
    }
    Ok(())
}
