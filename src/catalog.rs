//! One indexed upstream tool, identified as `server/tool`.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct ToolRecord {
    pub id: String,
    pub server: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
    /// Stable hash of name, description and schemas; a change between runs means the
    /// server altered the tool after it was first indexed.
    pub schema_hash: String,
}

impl ToolRecord {
    pub fn from_mcp(server: &str, tool: &rmcp::model::Tool) -> Self {
        let input_schema = Value::Object((*tool.input_schema).clone());
        let output_schema = tool
            .output_schema
            .as_ref()
            .map(|s| Value::Object((**s).clone()));
        let annotations = tool
            .annotations
            .as_ref()
            .and_then(|a| serde_json::to_value(a).ok());
        let description = tool.description.as_deref().unwrap_or_default().to_string();
        let hashed = serde_json::json!([&*tool.name, &description, &input_schema, &output_schema]);
        Self {
            id: format!("{server}/{}", tool.name),
            server: server.into(),
            name: tool.name.to_string(),
            description,
            input_schema,
            output_schema,
            annotations,
            schema_hash: format!("{:016x}", fnv1a(hashed.to_string().as_bytes())),
        }
    }

    /// (name, description, required) for each top-level input property.
    pub fn params(&self) -> Vec<(&str, &str, bool)> {
        let required: Vec<&str> = self.input_schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        self.input_schema["properties"]
            .as_object()
            .map(|props| {
                props
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.as_str(),
                            v["description"].as_str().unwrap_or(""),
                            required.contains(&k.as_str()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Property names of every object in the output schema (objects, arrays of objects), depth-limited.
    pub fn output_objects(&self) -> Vec<Vec<String>> {
        fn walk(v: &Value, depth: usize, out: &mut Vec<Vec<String>>) {
            if depth > 4 {
                return;
            }
            if let Some(props) = v["properties"].as_object() {
                out.push(props.keys().cloned().collect());
                props.values().for_each(|sub| walk(sub, depth + 1, out));
            }
            if v.get("items").is_some() {
                walk(&v["items"], depth + 1, out);
            }
        }
        let mut out = vec![];
        if let Some(s) = &self.output_schema {
            walk(s, 0, &mut out);
        }
        out
    }

    pub fn output_fields(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for f in self.output_objects().into_iter().flatten() {
            if !out.contains(&f) {
                out.push(f);
            }
        }
        out
    }

    /// True when some output object carries `field` next to a human-readable `name`/`title`,
    /// i.e. the tool resolves names to that id (a registry or lookup, not an incidental mention).
    pub fn resolves(&self, field: &str) -> bool {
        self.output_objects()
            .iter()
            .any(|o| o.iter().any(|f| f == field) && o.iter().any(|f| f == "name" || f == "title"))
    }

    pub fn destructive_hint(&self) -> bool {
        self.annotations
            .as_ref()
            .is_some_and(|a| a["destructiveHint"] == Value::Bool(true))
    }

    /// First sentence of the description, capped, for token-cheap discovery results.
    pub fn summary(&self) -> String {
        let first = self
            .description
            .split(". ")
            .next()
            .unwrap_or("")
            .trim_end_matches('.');
        let mut s: String = first.chars().take(140).collect();
        if s.len() < first.len() {
            s.push('…');
        }
        s
    }
}

/// FNV-1a 64: stable across Rust versions, unlike `DefaultHasher`, so stored hashes stay comparable.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;

    #[test]
    fn extracts_params_outputs_and_summary() {
        let schema = json!({"type":"object","properties":{"service_id":{"type":"string","description":"svc"},"limit":{"type":"integer"}},"required":["service_id"]});
        let out = json!({"type":"object","properties":{"deployments":{"type":"array","items":{"type":"object","properties":{"deployment_id":{"type":"string"},"commit_sha":{"type":"string"}}}}}});
        let tool = rmcp::model::Tool::new(
            "list_deployments",
            "List recent deployments. Newest first.",
            Arc::new(schema.as_object().unwrap().clone()),
        )
        .with_raw_output_schema(Arc::new(out.as_object().unwrap().clone()));
        let r = ToolRecord::from_mcp("deploy", &tool);
        assert_eq!(r.id, "deploy/list_deployments");
        let mut params = r.params();
        params.sort();
        assert_eq!(
            params,
            vec![("limit", "", false), ("service_id", "svc", true)]
        );
        assert_eq!(
            r.output_fields(),
            vec!["deployments", "commit_sha", "deployment_id"]
        );
        assert_eq!(r.summary(), "List recent deployments");
        assert_eq!(
            r.schema_hash,
            ToolRecord::from_mcp("deploy", &tool).schema_hash
        );
    }
}
