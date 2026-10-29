//! End-to-end over real MCP stdio: an rmcp client spawns `toollens serve`, which spawns the
//! three fixture servers. Proves the three-tool contract, policy separation and validation.

use std::process::Stdio;

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
};
use serde_json::{Value, json};

async fn call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &str,
    args: Value,
) -> CallToolResult {
    let params = CallToolRequestParams::new(name.to_string())
        .with_arguments(args.as_object().unwrap().clone());
    client
        .call_tool(params)
        .await
        .expect("transport-level success")
}

fn body(r: &CallToolResult) -> Value {
    r.structured_content.clone().expect("structured content")
}

#[tokio::test]
async fn gateway_exposes_three_tools_and_separates_discovery_from_permission() {
    let db = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_toollens"));
    cmd.args([
        "serve",
        "--config",
        concat!(env!("CARGO_MANIFEST_DIR"), "/examples/gateway.json"),
        "--noise",
        "150",
        "--db",
    ])
    .arg(db.path().join("t.db"));
    let (transport, _) = TokioChildProcess::builder(cmd)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let client = ().serve(transport).await.unwrap();

    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, ["discover_tools", "inspect_tool", "invoke_tool"]);
    assert!(
        tools[0]
            .description
            .as_deref()
            .unwrap()
            .contains("168 tools"),
        "18 fixture + 150 noise"
    );

    // Discovery: dependency-aware, compact (no schemas), and grants nothing.
    let d = body(
        &call(
            &client,
            "discover_tools",
            json!({"goal": "investigate a failed deployment"}),
        )
        .await,
    );
    let ids: Vec<&str> = d["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    for want in [
        "deploy/get_deployment",
        "deploy/search",
        "observability/get_logs",
        "vcs/get_commit",
    ] {
        assert!(ids.contains(&want), "{want} missing from {ids:?}");
    }
    assert!(ids.len() <= 8);
    assert!(
        !d.to_string().contains("inputSchema"),
        "discovery stays token-cheap"
    );

    // Inspect shows schema, provenance and the policy decision.
    let i = body(
        &call(
            &client,
            "inspect_tool",
            json!({"tool_id": "deploy/rollback"}),
        )
        .await,
    );
    assert_eq!(i["permission"]["decision"], "deny");
    assert_eq!(i["provenance"]["server"], "deploy");
    assert!(i["input_schema"]["required"].as_array().unwrap().len() == 2);

    // Finding a tool never grants permission: rollback is denied and not executed.
    let r = call(&client, "invoke_tool", json!({"tool_id": "deploy/rollback", "arguments": {"service_id": "svc-checkout", "deployment_id": "dep-4790"}})).await;
    assert_eq!(r.is_error, Some(true));
    assert_eq!(body(&r)["outcome"], "denied");

    // Destructive noise tools are denied by annotation, unless explicitly allowed.
    let found = tools_matching(&client, "permanently delete a contact").await;
    let id = found
        .iter()
        .find(|id| id.ends_with("/delete_contact"))
        .expect("noise catalog has delete_contact");
    let r = call(
        &client,
        "invoke_tool",
        json!({"tool_id": id, "arguments": {"contact_id": "c-1"}}),
    )
    .await;
    assert_eq!(body(&r)["outcome"], "denied", "{id}");
    assert!(
        body(&r)["error"]
            .as_str()
            .unwrap()
            .contains("denyDestructive")
    );

    // Arguments are validated before anything reaches the upstream server.
    let r = call(
        &client,
        "invoke_tool",
        json!({"tool_id": "deploy/list_deployments", "arguments": {"limit": 0}}),
    )
    .await;
    assert_eq!(body(&r)["outcome"], "invalid");
    assert!(body(&r)["error"].as_str().unwrap().contains("service_id"));

    // Unknown ids are rejected with guidance.
    let r = call(
        &client,
        "invoke_tool",
        json!({"tool_id": "deploy/nope", "arguments": {}}),
    )
    .await;
    assert_eq!(body(&r)["outcome"], "unknown");

    // A valid call runs on the owning server; overlapping names resolve by full id.
    let r = call(
        &client,
        "invoke_tool",
        json!({"tool_id": "vcs/get_status", "arguments": {"repo": "acme/checkout-api"}}),
    )
    .await;
    assert_ne!(r.is_error, Some(true));
    assert_eq!(body(&r)["ci"]["state"], "passing");
    let r = call(
        &client,
        "invoke_tool",
        json!({"tool_id": "deploy/get_status", "arguments": {"service_id": "svc-checkout"}}),
    )
    .await;
    assert_eq!(body(&r)["serving"]["version"], "v2.13.2");

    // Upstream tool errors pass through as tool errors, not gateway rejections.
    let r = call(
        &client,
        "invoke_tool",
        json!({"tool_id": "deploy/list_deployments", "arguments": {"service_id": "checkout-api"}}),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    assert!(r.structured_content.is_none());

    client.cancel().await.unwrap();
}

async fn tools_matching(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    goal: &str,
) -> Vec<String> {
    let d = body(&call(client, "discover_tools", json!({"goal": goal, "limit": 10})).await);
    d["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
}
