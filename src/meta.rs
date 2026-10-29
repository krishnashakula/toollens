//! The three tools ToolLens exposes, shared by the MCP gateway and the benchmark agent
//! so both exercise exactly the same contract.

use rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::{
    discover::MAX_LIMIT,
    lens::{DiscoverOpts, Invocation, Lens},
};

pub const DISCOVER: &str = "discover_tools";
pub const INSPECT: &str = "inspect_tool";
pub const INVOKE: &str = "invoke_tool";

pub struct Spec {
    pub name: &'static str,
    pub description: String,
    pub input_schema: Value,
}

pub fn specs(lens: &Lens) -> Vec<Spec> {
    let n = lens.tools().len();
    let servers = lens.servers().len();
    vec![
        Spec {
            name: DISCOVER,
            description: format!(
                "Find tools for a task in this gateway's catalog of {n} tools from {servers} MCP servers. \
                 Describe the goal in plain words (what you need to find out or do), not a guessed tool name. \
                 Returns up to `limit` tool ids with one-line summaries: direct matches plus the supporting \
                 tools they depend on (for example a tool that provides an id another tool requires). \
                 Call it again with a narrower goal if a step needs a tool you do not have yet. \
                 Discovery never grants permission to run a tool."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "goal": {"type": "string", "description": "What you are trying to accomplish."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Max tools to return (default 8)."}
                },
                "required": ["goal"],
                "additionalProperties": false
            }),
        },
        Spec {
            name: INSPECT,
            description: "Get one tool's full input and output schema, which server provides it, and whether \
                          the gateway policy currently allows running it. Use it before invoke_tool to get \
                          argument names and types exactly right."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {"tool_id": {"type": "string", "description": "A tool id from discover_tools, e.g. `server/tool_name`."}},
                "required": ["tool_id"],
                "additionalProperties": false
            }),
        },
        Spec {
            name: INVOKE,
            description: "Run one tool by id. The gateway checks its permission policy and validates the \
                          arguments against the tool's input schema before forwarding the call to the server \
                          that owns it. Denied or invalid calls are not executed and return an error explaining why."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tool_id": {"type": "string", "description": "A tool id from discover_tools."},
                    "arguments": {"type": "object", "description": "Arguments matching the tool's input schema."}
                },
                "required": ["tool_id", "arguments"],
                "additionalProperties": false
            }),
        },
    ]
}

pub enum Output {
    /// A gateway-produced JSON payload.
    Json { value: Value, is_error: bool },
    /// An upstream tool result, passed through unchanged.
    Upstream(CallToolResult),
}

impl Output {
    fn error(message: impl Into<String>) -> Self {
        Output::Json {
            value: json!({"error": message.into()}),
            is_error: true,
        }
    }

    pub fn into_call_result(self) -> CallToolResult {
        match self {
            Output::Upstream(r) => r,
            Output::Json {
                value,
                is_error: false,
            } => CallToolResult::structured(value),
            Output::Json {
                value,
                is_error: true,
            } => CallToolResult::structured_error(value),
        }
    }

    /// Text form for model APIs that take a string tool result.
    pub fn to_text(&self) -> (String, bool) {
        match self {
            Output::Json { value, is_error } => (value.to_string(), *is_error),
            Output::Upstream(r) => {
                let text = match &r.structured_content {
                    Some(v) => v.to_string(),
                    None => r
                        .content
                        .iter()
                        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                (text, r.is_error == Some(true))
            }
        }
    }
}

pub async fn dispatch(
    lens: &Lens,
    session: &str,
    opts: DiscoverOpts,
    name: &str,
    args: &Value,
) -> Output {
    let str_arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    match name {
        DISCOVER => {
            let Some(goal) = str_arg("goal") else {
                return Output::error("`goal` (string) is required");
            };
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map_or(opts.limit, |l| l as usize);
            match lens
                .discover(session, &goal, DiscoverOpts { limit, ..opts })
                .await
            {
                Ok(d) => Output::Json {
                    value: json!({"tools": d.tools, "catalog_size": d.catalog_size}),
                    is_error: false,
                },
                Err(e) => Output::error(format!("discovery failed: {e:#}")),
            }
        }
        INSPECT => {
            let Some(id) = str_arg("tool_id") else {
                return Output::error("`tool_id` (string) is required");
            };
            match lens.inspect(session, &id) {
                Ok(i) => Output::Json {
                    value: serde_json::to_value(&i).unwrap_or_default(),
                    is_error: false,
                },
                Err(e) => Output::error(format!("{e:#}")),
            }
        }
        INVOKE => {
            let Some(id) = str_arg("tool_id") else {
                return Output::error("`tool_id` (string) is required");
            };
            match lens
                .invoke(
                    session,
                    &id,
                    args.get("arguments").cloned().unwrap_or(Value::Null),
                )
                .await
            {
                Invocation::Executed(r) => Output::Upstream(r),
                Invocation::Rejected { outcome, message } => Output::Json {
                    value: json!({"error": message, "outcome": outcome}),
                    is_error: true,
                },
            }
        }
        other => Output::error(format!(
            "unknown tool `{other}`; this gateway exposes {DISCOVER}, {INSPECT}, {INVOKE}"
        )),
    }
}
