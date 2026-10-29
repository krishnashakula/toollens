//! Lazy mode end to end with real fixture processes: index once, restart from cache without
//! starting anything, start servers on first use, and stop the least recently used one.

use serde_json::json;
use toollens::{
    config::Config,
    lens::{Invocation, Lens},
};

fn config() -> Config {
    let exe = env!("CARGO_BIN_EXE_toollens");
    let server = |kind: &str| json!({"command": exe, "args": ["fixture", kind]});
    serde_json::from_value(json!({
        "mcpServers": {"deploy": server("deploy"), "observability": server("observability"), "vcs": server("vcs")},
        "policy": {"default": "allow"},
        "lazy": true,
        "maxLive": 1
    }))
    .unwrap()
}

fn ok(inv: Invocation) -> serde_json::Value {
    match inv {
        Invocation::Executed(r) if r.is_error != Some(true) => r.structured_content.unwrap(),
        Invocation::Executed(r) => panic!("tool error: {:?}", r.content),
        Invocation::Rejected { outcome, message } => panic!("{outcome}: {message}"),
    }
}

#[tokio::test]
async fn lazy_pool_indexes_once_starts_on_demand_and_evicts_lru() {
    let db = tempfile::tempdir().unwrap();
    let db = db.path().join("lazy.db");

    let first = Lens::start(&config(), &db).await.unwrap();
    assert_eq!(first.tools().len(), 18);
    assert_eq!(first.indexed_from_cache, 0);
    assert_eq!(
        first.live_servers(),
        0,
        "lazy indexing stops servers after listing"
    );
    drop(first);

    let lens = Lens::start(&config(), &db).await.unwrap();
    assert_eq!(
        lens.indexed_from_cache, 3,
        "unchanged configs come from the cache"
    );
    assert_eq!(lens.tools().len(), 18);
    assert_eq!(lens.live_servers(), 0);

    let v = ok(lens
        .invoke(
            "t",
            "deploy/get_status",
            json!({"service_id": "svc-checkout"}),
        )
        .await);
    assert_eq!(v["serving"]["version"], "v2.13.2");
    assert_eq!(lens.live_servers(), 1);

    let v = ok(lens
        .invoke("t", "vcs/get_status", json!({"repo": "acme/checkout-api"}))
        .await);
    assert_eq!(v["ci"]["state"], "passing");
    assert_eq!(lens.live_servers(), 1, "maxLive 1: deploy was stopped");

    // A stopped server restarts transparently on its next call.
    let v = ok(lens
        .invoke(
            "t",
            "deploy/list_deployments",
            json!({"service_id": "svc-search"}),
        )
        .await);
    assert_eq!(v["deployments"][0]["deployment_id"], "dep-4818");

    let mut changed = config();
    changed
        .mcp_servers
        .get_mut("vcs")
        .unwrap()
        .args
        .push("--seed".into());
    changed
        .mcp_servers
        .get_mut("vcs")
        .unwrap()
        .args
        .push("1".into());
    let relisted = Lens::start(&changed, &db).await.unwrap();
    assert_eq!(
        relisted.indexed_from_cache, 2,
        "a changed config is re-indexed"
    );
}
