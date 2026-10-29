//! Live agent sessions for the console: a real Claude conversation through the gateway (or with
//! every tool exposed), streamed step by step and saved to disk so a run can be shown again
//! without paying for it twice.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{App, err};
use crate::bench::{
    agent::{self, Arm},
    claude::Claude,
};

const MAX_IN_MEMORY: usize = 16;
const MAX_PROMPT_CHARS: usize = 2000;

type Session = Arc<Mutex<Value>>;

pub struct Live {
    claude: Option<Claude>,
    model: String,
    arms: tokio::sync::Mutex<HashMap<String, Arc<Arm>>>,
    sessions: Mutex<VecDeque<(String, Session)>>,
    dir: PathBuf,
    /// One live run at a time: each one spends real API money.
    running: AtomicBool,
}

/// Clears `running` however the run ends, panics included.
struct Running<'a>(&'a AtomicBool);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Live {
    /// Without ANTHROPIC_API_KEY the console still works; only live runs are refused.
    pub fn new(model: String, dir: PathBuf) -> Self {
        let claude = Claude::from_env()
            .inspect_err(|e| tracing::warn!(error = %e, "live runs disabled"))
            .ok();
        Self {
            claude,
            model,
            arms: Default::default(),
            sessions: Default::default(),
            dir,
            running: AtomicBool::new(false),
        }
    }

    /// Tool definitions per arm are built once (one count_tokens call) and reused.
    async fn arm(&self, app: &App, claude: &Claude, name: &str) -> anyhow::Result<Arc<Arm>> {
        let mut arms = self.arms.lock().await;
        if let Some(a) = arms.get(name) {
            return Ok(a.clone());
        }
        let arm = Arc::new(agent::build_arm(claude, &app.lens, name, &self.model).await?);
        arms.insert(name.to_string(), arm.clone());
        Ok(arm)
    }

    fn find(&self, id: &str) -> Option<Session> {
        let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        sessions
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, s)| s.clone())
    }

    fn keep(&self, id: &str, s: Session) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        if sessions.len() == MAX_IN_MEMORY {
            sessions.pop_front();
        }
        sessions.push_back((id.to_string(), s));
    }
}

#[derive(Deserialize)]
pub struct StartReq {
    prompt: String,
    arm: String,
}

pub async fn start(State(app): State<App>, Json(req): Json<StartReq>) -> Response {
    let live = app.live.clone();
    let Some(claude) = live.claude.as_ref() else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "live runs need ANTHROPIC_API_KEY",
        );
    };
    let prompt = req.prompt.trim().to_string();
    if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
        return err(StatusCode::BAD_REQUEST, "prompt must be 1-2000 characters");
    }
    if !matches!(req.arm.as_str(), "lens" | "full") {
        return err(StatusCode::BAD_REQUEST, "arm must be lens or full");
    }
    let arm = match live.arm(&app, claude, &req.arm).await {
        Ok(a) => a,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    };
    if live.running.swap(true, Ordering::SeqCst) {
        return err(StatusCode::CONFLICT, "a live run is already in progress");
    }
    let unix = crate::bench::unix_now();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let id = format!("live-{unix}-{nanos:09}");
    let session: Session = Arc::new(Mutex::new(json!({
        "id": id, "prompt": prompt, "arm": req.arm, "model": live.model, "started_unix": unix,
        "tools_in_catalog": app.lens.tools().len(), "tool_defs_sent": arm.tool_count(),
        "schema_tokens_per_request": arm.schema_tokens(), "events": [], "done": false,
    })));
    live.keep(&id, session.clone());
    let (lens, run_id) = (app.lens.clone(), id.clone());
    tokio::spawn(async move {
        let _running = Running(&live.running);
        let claude = live.claude.as_ref().expect("checked above");
        let on_step = |step: &Value| {
            let mut event = step.clone();
            if step["kind"] == "tool" {
                // What the gateway did for this call, from its own audit log.
                if let Ok(Some(c)) = lens
                    .store
                    .calls(Some(&run_id), 1)
                    .map(|v| v.into_iter().next())
                {
                    event["gateway"] = json!({"kind": c.kind, "target": c.target, "outcome": c.outcome, "latency_ms": c.latency_ms});
                }
            }
            push(&session, event);
        };
        let conv =
            agent::converse(claude, &lens, &arm, &run_id, &prompt, &live.model, &on_step).await;
        let m = &conv.metrics;
        push(
            &session,
            json!({"kind": "final", "text": conv.final_text, "stop_reason": conv.stop_reason, "error": conv.error,
                   "requests": m.requests, "input_tokens": m.input_tokens, "output_tokens": m.output_tokens,
                   "elapsed_ms": m.wall_ms.round()}),
        );
        let snapshot = {
            let mut s = session.lock().unwrap_or_else(|p| p.into_inner());
            s["done"] = json!(true);
            s.clone()
        };
        let path = live.dir.join(format!("{run_id}.json"));
        let saved = std::fs::create_dir_all(&live.dir).and_then(|_| {
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&snapshot).unwrap_or_default(),
            )
        });
        if let Err(e) = saved {
            tracing::error!(path = %path.display(), error = %e, "could not save live session");
        }
    });
    Json(json!({"id": id})).into_response()
}

fn push(session: &Session, event: Value) {
    let mut s = session.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(events) = s["events"].as_array_mut() {
        events.push(event);
    }
}

fn valid_id(id: &str) -> bool {
    id.starts_with("live-")
        && id.len() < 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub async fn get(State(app): State<App>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return err(StatusCode::BAD_REQUEST, "bad session id");
    }
    if let Some(s) = app.live.find(&id) {
        return Json(s.lock().unwrap_or_else(|p| p.into_inner()).clone()).into_response();
    }
    match std::fs::read_to_string(app.live.dir.join(format!("{id}.json"))) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) => Json(v).into_response(),
            Err(e) => err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("unreadable session: {e}"),
            ),
        },
        Err(_) => err(StatusCode::NOT_FOUND, "no such session"),
    }
}

/// Saved sessions, newest first, without their events.
pub async fn list(State(app): State<App>) -> Json<Value> {
    let mut out: Vec<Value> = std::fs::read_dir(&app.live.dir)
        .into_iter()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
        .filter_map(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| json!({"id": v["id"], "prompt": v["prompt"], "arm": v["arm"], "started_unix": v["started_unix"]}))
        .collect();
    out.sort_by(|a, b| b["started_unix"].as_u64().cmp(&a["started_unix"].as_u64()));
    Json(Value::Array(out))
}

#[derive(Deserialize)]
pub struct WarmReq {
    servers: Vec<String>,
}

/// Starts servers ahead of use, as a production deployment keeps hot servers warm.
pub async fn warm(State(app): State<App>, Json(req): Json<WarmReq>) -> Json<Value> {
    let mut out = serde_json::Map::new();
    for s in req.servers.iter().take(32) {
        let r = app.lens.warm(s).await;
        out.insert(
            s.clone(),
            json!(
                r.err()
                    .map(|e| format!("{e:#}"))
                    .unwrap_or_else(|| "ready".into())
            ),
        );
    }
    Json(Value::Object(out))
}
