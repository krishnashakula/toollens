//! Local web console: live agent runs and live discovery against the running gateway, replays
//! of recorded benchmark runs, and benchmark charts. Binds to 127.0.0.1 only, and every `/api`
//! call must carry the token printed at startup.

mod live;

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::lens::{DiscoverOpts, Lens, Mode};

const INDEX: &str = include_str!("index.html");

#[derive(Clone)]
struct App {
    lens: Arc<Lens>,
    results: PathBuf,
    live: Arc<live::Live>,
}

pub async fn serve(
    lens: Arc<Lens>,
    results: PathBuf,
    port: u16,
    model: String,
) -> anyhow::Result<()> {
    let live = Arc::new(live::Live::new(model, results.join("live")));
    let token: Arc<str> = random_token().into();
    let app = Router::new()
        .route("/api/catalog", get(catalog))
        .route("/api/discover", post(discover))
        .route("/api/runs", get(runs))
        .route("/api/run", get(run))
        .route("/api/bench", get(bench))
        .route("/api/audit", get(audit))
        .route("/api/live", get(live::list).post(live::start))
        .route("/api/live/{id}", get(live::get))
        .route("/api/warm", post(live::warm))
        // Binding to 127.0.0.1 is not enough: Docker containers reach it through
        // host.docker.internal and web pages through DNS rebinding, and /api/live spends the
        // user's API credits. Like Jupyter, the token is shown only in this terminal.
        .route_layer(middleware::from_fn_with_state(token.clone(), require_token))
        .route("/", get(|| async { Html(INDEX) }))
        .with_state(App {
            lens,
            results,
            live,
        });
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // The URL is how the user gets the token, so it goes to the terminal, not to the log.
    eprintln!("\n  ToolLens console: http://{addr}/#token={token}\n");
    axum::serve(listener, app).await?;
    Ok(())
}

/// 128 bits from std's OS-seeded SipHash keys, so no RNG dependency.
/// shortcut: fine for a local session token; use a CSPRNG crate if tokens ever outlive the process.
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let keys = std::collections::hash_map::RandomState::new();
    let part = |i: u64| {
        let mut h = keys.build_hasher();
        h.write_u64(i);
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

async fn require_token(State(token): State<Arc<str>>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get("x-toollens-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // Constant time: the comparison must not reveal how much of a guess was right.
    let same = given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if same {
        next.run(req).await
    } else {
        err(
            StatusCode::UNAUTHORIZED,
            "missing or wrong console token: open the URL the console printed",
        )
    }
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

async fn catalog(State(app): State<App>) -> Json<Value> {
    let lens = &app.lens;
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for t in lens.tools() {
        by_name
            .entry(t.name.as_str())
            .or_default()
            .push(t.server.as_str());
    }
    let mut overlaps: Vec<_> = by_name.into_iter().filter(|(_, s)| s.len() > 1).collect();
    overlaps.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    Json(json!({
        "tools": lens.tools().len(),
        "servers": lens.servers().into_iter().map(|(name, n)| json!({"name": name, "tools": n})).collect::<Vec<_>>(),
        "unavailable": lens.unavailable,
        "overlaps": overlaps.iter().take(12).map(|(n, s)| json!({"name": n, "servers": s})).collect::<Vec<_>>(),
        "ids": lens.tools().iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct DiscoverReq {
    goal: String,
    mode: Option<Mode>,
    deps: Option<bool>,
    limit: Option<usize>,
}

async fn discover(State(app): State<App>, Json(req): Json<DiscoverReq>) -> Response {
    let d = DiscoverOpts::default();
    let opts = DiscoverOpts {
        limit: req.limit.unwrap_or(d.limit),
        mode: req.mode.unwrap_or(d.mode),
        deps: req.deps.unwrap_or(d.deps),
    };
    match app.lens.discover("console", &req.goal, opts).await {
        Ok(d) => Json(d).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

/// Results of one benchmark set: `synthetic` (fixture servers plus noise) or `real` (bench/real).
fn set_dir(app: &App, set: Option<&str>) -> Option<PathBuf> {
    match set.unwrap_or("synthetic") {
        "synthetic" => Some(app.results.clone()),
        "real" => Some(app.results.join("real")),
        _ => None,
    }
}

#[derive(Deserialize)]
struct SetQuery {
    set: Option<String>,
}

async fn runs(State(app): State<App>, Query(q): Query<SetQuery>) -> Response {
    let Some(dir) = set_dir(&app, q.set.as_deref()) else {
        return err(StatusCode::BAD_REQUEST, "unknown set");
    };
    let mut out = vec![];
    let root = dir.join("agent");
    for size in read_dir_names(&root) {
        for arm in read_dir_names(&root.join(&size)) {
            for file in read_dir_names(&root.join(&size).join(&arm)) {
                if let Some(task) = file.strip_suffix(".json") {
                    out.push(json!({"size": size.parse::<usize>().unwrap_or(0), "arm": arm, "task": task}));
                }
            }
        }
    }
    Json(Value::Array(out)).into_response()
}

#[derive(Deserialize)]
struct RunQuery {
    size: usize,
    arm: String,
    task: String,
    set: Option<String>,
}

async fn run(State(app): State<App>, Query(q): Query<RunQuery>) -> Response {
    // Path components come from the URL: allow only known sets, arms and slug-shaped task ids.
    let slug = !q.task.is_empty()
        && q.task
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let Some(dir) = set_dir(&app, q.set.as_deref()) else {
        return err(StatusCode::BAD_REQUEST, "unknown set");
    };
    if !matches!(q.arm.as_str(), "full" | "lens" | "native") || !slug {
        return err(StatusCode::BAD_REQUEST, "bad arm or task id");
    }
    let path = dir
        .join("agent")
        .join(q.size.to_string())
        .join(&q.arm)
        .join(format!("{}.json", q.task));
    match read_json(&path) {
        Some(v) => Json(v).into_response(),
        None => err(StatusCode::NOT_FOUND, "no such run"),
    }
}

async fn bench(State(app): State<App>) -> Json<Value> {
    let real = app.results.join("real");
    Json(json!({
        "agent": read_json(&app.results.join("agent.json")),
        "retrieval": read_json(&app.results.join("retrieval.json")),
        "real_agent": read_json(&real.join("agent.json")),
        "real_retrieval": read_json(&real.join("retrieval.json")),
        // results dir is bench/results by default; the index report lives in bench/real.
        "real_index": app.results.parent().and_then(|b| read_json(&b.join("real").join("index-report.json"))),
    }))
}

#[derive(Deserialize)]
struct AuditQuery {
    limit: Option<usize>,
}

async fn audit(State(app): State<App>, Query(q): Query<AuditQuery>) -> Response {
    match app.lens.store.calls(None, q.limit.unwrap_or(20).min(200)) {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "unreadable results file");
            None
        }
    }
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    v.sort();
    v
}
