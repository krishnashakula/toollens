//! Argument validation against the upstream tool's JSON Schema, before anything is forwarded.

use serde_json::Value;

const MAX_ERRORS: usize = 5;

/// Fails closed: a schema that does not compile rejects every call rather than letting it through.
pub fn check_args(schema: &Value, args: &Value) -> Result<(), Vec<String>> {
    if !args.is_object() {
        return Err(vec!["arguments must be a JSON object".into()]);
    }
    let validator = jsonschema::validator_for(schema).map_err(|e| {
        vec![format!(
            "upstream tool has an invalid input schema, refusing to call it: {e}"
        )]
    })?;
    let errors: Vec<String> = validator
        .iter_errors(args)
        .take(MAX_ERRORS)
        .map(|e| {
            let path = e.instance_path().to_string();
            if path.is_empty() {
                e.to_string()
            } else {
                format!("{path}: {e}")
            }
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Value {
        json!({"type":"object","properties":{"service_id":{"type":"string"},"limit":{"type":"integer","minimum":1}},"required":["service_id"],"additionalProperties":false})
    }

    #[test]
    fn accepts_valid_and_reports_each_problem() {
        assert!(check_args(&schema(), &json!({"service_id": "svc-1", "limit": 3})).is_ok());
        let errs = check_args(&schema(), &json!({"limit": 0, "extra": true})).unwrap_err();
        assert_eq!(errs.len(), 3, "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("service_id")));
        assert!(errs.iter().any(|e| e.starts_with("/limit")));
    }

    #[test]
    fn rejects_non_objects_and_broken_schemas() {
        assert!(check_args(&schema(), &json!([1])).is_err());
        assert!(check_args(&json!({"type": 12}), &json!({})).is_err());
    }
}
