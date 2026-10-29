//! Simulated upstream MCP servers with deterministic data, so benchmarks are reproducible:
//! `deploy`, `observability` and `vcs` (overlapping tool names, one shared incident story)
//! and `noise` (generated distractor tools).

pub mod infra;
pub mod noise;

use std::sync::Arc;

use anyhow::bail;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Map, Value};

pub type Args = Map<String, Value>;
pub type Handler = Arc<dyn Fn(&str, &Args) -> Result<Value, String> + Send + Sync>;

#[derive(Clone)]
pub struct Fixture {
    tools: Vec<Tool>,
    handler: Handler,
}

impl Fixture {
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name.to_string()).collect()
    }
}

impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        let result = match (self.handler)(&request.name, &args) {
            Ok(v) => CallToolResult::structured(v),
            Err(msg) => CallToolResult::error(vec![ContentBlock::text(msg)]),
        };
        Ok(result.into())
    }
}

pub async fn serve(kind: &str, count: usize, seed: usize) -> anyhow::Result<()> {
    let fixture = match kind {
        "deploy" => infra::deploy(),
        "observability" => infra::observability(),
        "vcs" => infra::vcs(),
        "noise" => noise::server(count, seed),
        other => bail!("unknown fixture {other:?} (deploy | observability | vcs | noise)"),
    };
    fixture
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

pub fn tool(
    name: &str,
    description: &str,
    input: Value,
    output: Option<Value>,
    destructive: bool,
) -> Tool {
    let obj = |v: Value| Arc::new(v.as_object().cloned().expect("schema literal is an object"));
    let mut t = Tool::new(name.to_string(), description.to_string(), obj(input)).with_annotations(
        ToolAnnotations::new()
            .read_only(!destructive)
            .destructive(destructive),
    );
    if let Some(o) = output {
        t = t.with_raw_output_schema(obj(o));
    }
    t
}

pub fn arg<'a>(args: &'a Args, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}
