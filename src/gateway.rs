//! MCP server face of the gateway: exactly three tools, whatever the catalog size.

use std::sync::Arc;

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};

use crate::{
    lens::{DiscoverOpts, Lens},
    meta,
};

const INSTRUCTIONS: &str = "This gateway fronts many MCP servers. Do not guess tool names: call \
discover_tools with your goal, inspect_tool for the exact schema, then invoke_tool. Tool ids look like \
`server/tool`; several servers may offer tools with the same name, so always use the full id.";

#[derive(Clone)]
pub struct Gateway {
    lens: Arc<Lens>,
    opts: DiscoverOpts,
    session: String,
}

impl Gateway {
    pub fn new(lens: Arc<Lens>, opts: DiscoverOpts) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self {
            lens,
            opts,
            session: format!("mcp-{}-{nanos}", std::process::id()),
        }
    }

    fn tools(&self) -> Vec<Tool> {
        meta::specs(&self.lens)
            .into_iter()
            .map(|s| {
                let schema = s.input_schema.as_object().cloned().unwrap_or_default();
                Tool::new(s.name, s.description, Arc::new(schema))
            })
            .collect()
    }
}

impl ServerHandler for Gateway {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("toollens", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = serde_json::Value::Object(request.arguments.unwrap_or_default());
        // rmcp only flags a client's cancellation on `context.ct`; without this, a call waiting on
        // a server start or a permit would still run after the user cancelled it. Dropping the
        // dispatch drops the upstream call, which cancels it there too.
        tokio::select! {
            out = meta::dispatch(&self.lens, &self.session, self.opts, &request.name, &args) => {
                Ok(out.into_call_result().into())
            }
            () = context.ct.cancelled() => Err(ErrorData::internal_error("request cancelled by the client", None)),
        }
    }
}
