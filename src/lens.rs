//! The gateway engine: catalog, retrieval, dependency graph, policy and upstream calls.
//! The MCP server, the console and the benchmark all drive this same type.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    str::FromStr,
    sync::{Mutex, RwLock},
    time::Instant,
};

use anyhow::{bail, ensure};
use rmcp::model::CallToolResult;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::{
    catalog::ToolRecord,
    config::Config,
    discover::{self, Role},
    graph::{Edge, Observed, SchemaGraph, SessionMemory},
    keyword::{self, Bm25},
    policy::{Policy, Verdict},
    semantic::{self, Embedder},
    store::{Provenance, Store},
    text::tokens,
    upstream::{self, Pool},
    validate::check_args,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Keyword,
    Semantic,
    Hybrid,
}

impl FromStr for Mode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "keyword" => Mode::Keyword,
            "semantic" => Mode::Semantic,
            "hybrid" => Mode::Hybrid,
            _ => bail!("unknown mode {s:?} (keyword | semantic | hybrid)"),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiscoverOpts {
    pub limit: usize,
    pub mode: Mode,
    pub deps: bool,
}

impl Default for DiscoverOpts {
    fn default() -> Self {
        Self {
            limit: 8,
            mode: Mode::Keyword,
            deps: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub id: String,
    pub summary: String,
    pub role: Role,
    pub why: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Discovery {
    pub tools: Vec<Hit>,
    pub edges: Vec<Edge>,
    pub catalog_size: usize,
    pub latency_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct Inspection<'a> {
    #[serde(flatten)]
    pub tool: &'a ToolRecord,
    pub provenance: Option<Provenance>,
    /// Current policy decision, informational only: invoke re-checks on every call.
    pub permission: Verdict,
}

pub enum Invocation {
    /// The upstream server ran the tool (its result may still be a tool-level error).
    Executed(CallToolResult),
    /// The gateway did not forward the call.
    Rejected {
        outcome: &'static str,
        message: String,
    },
}

pub struct Lens {
    tools: Vec<ToolRecord>,
    by_id: HashMap<String, usize>,
    bm25: Bm25,
    graph: SchemaGraph,
    observed: RwLock<Observed>,
    memory: Mutex<SessionMemory>,
    vectors: OnceCell<Vec<Vec<f32>>>,
    embedder: Embedder,
    policy: Policy,
    pool: Pool,
    pub unavailable: Vec<(String, String)>,
    /// Servers whose tool list came from the index cache at startup (lazy mode).
    pub indexed_from_cache: usize,
    pub store: Store,
    learn: bool,
}

impl Lens {
    /// Indexes every upstream server's tools (from cache in lazy mode) and builds the catalog.
    pub async fn start(cfg: &Config, db: &Path) -> anyhow::Result<Self> {
        let store = Store::open(db)?;
        let max_live = if cfg.lazy { cfg.max_live } else { usize::MAX };
        let pool = Pool::new(cfg.mcp_servers.clone(), max_live);
        let index = upstream::index(cfg, &store, &pool).await;
        ensure!(
            !index.servers.is_empty(),
            "no upstream server could be indexed: {:?}",
            index.failed
        );
        tracing::info!(
            servers = index.servers.len(),
            from_cache = index.from_cache,
            failed = index.failed.len(),
            live = pool.live_count(),
            "catalog indexed"
        );
        let mut tools = vec![];
        for (server, list) in &index.servers {
            let command = upstream::command_line(&cfg.mcp_servers[server]);
            for t in list {
                let rec = ToolRecord::from_mcp(server, t);
                store.record_tool(&rec, &command)?;
                tools.push(rec);
            }
        }
        let mut lens = Self::new(tools, pool, index.failed, cfg.policy.clone(), store);
        lens.indexed_from_cache = index.from_cache;
        Ok(lens)
    }

    pub fn new(
        mut tools: Vec<ToolRecord>,
        pool: Pool,
        unavailable: Vec<(String, String)>,
        policy: Policy,
        store: Store,
    ) -> Self {
        let mut by_id = HashMap::new();
        tools.retain(|t| {
            let fresh = !by_id.contains_key(&t.id);
            if fresh {
                by_id.insert(t.id.clone(), by_id.len());
            } else {
                tracing::warn!(id = %t.id, "server listed the same tool twice; keeping the first");
            }
            fresh
        });
        let bm25 = Bm25::build(tools.iter().map(keyword::document));
        let graph = SchemaGraph::build(&tools);
        let observed = Observed::from_rows(store.observed_edges().unwrap_or_else(|e| {
            tracing::error!(error = %e, "could not load observed edges; starting with none");
            vec![]
        }));
        Self {
            tools,
            by_id,
            bm25,
            graph,
            observed: RwLock::new(observed),
            memory: Mutex::new(SessionMemory::default()),
            vectors: OnceCell::new(),
            embedder: Embedder::from_env(),
            policy,
            pool,
            unavailable,
            indexed_from_cache: 0,
            store,
            learn: true,
        }
    }

    /// Turn learning of observed edges on or off (benchmarks run with a fixed, cold graph).
    pub fn learning(mut self, on: bool) -> Self {
        self.learn = on;
        if !on {
            // Fixed means fixed: no edges learned earlier in the same store (e.g. console runs).
            self.observed = RwLock::new(Observed::default());
        }
        self
    }

    /// Starts `server` now instead of on its first call (lazy mode); no-op if it is running.
    pub async fn warm(&self, server: &str) -> anyhow::Result<()> {
        self.pool.get(server).await.map(|_| ())
    }

    /// Upstream servers currently running.
    pub fn live_servers(&self) -> usize {
        self.pool.live_count()
    }

    pub fn tools(&self) -> &[ToolRecord] {
        &self.tools
    }

    pub fn tool(&self, id: &str) -> Option<&ToolRecord> {
        self.by_id.get(id).map(|i| &self.tools[*i])
    }

    pub fn servers(&self) -> BTreeMap<&str, usize> {
        let mut m = BTreeMap::new();
        for t in &self.tools {
            *m.entry(t.server.as_str()).or_default() += 1;
        }
        m
    }

    /// Embeds the whole catalog (cached in SQLite). Call before timing semantic discovery.
    pub async fn prepare_semantic(&self) -> anyhow::Result<&[Vec<f32>]> {
        let v = self
            .vectors
            .get_or_try_init(|| async {
                let texts: Vec<String> = self
                    .tools
                    .iter()
                    .map(|t| Embedder::doc_text(&t.id.replace(['_', '/'], " "), &t.description))
                    .collect();
                self.embedder.embed(&self.store, &texts).await
            })
            .await?;
        Ok(v)
    }

    pub async fn discover(
        &self,
        session: &str,
        goal: &str,
        opts: DiscoverOpts,
    ) -> anyhow::Result<Discovery> {
        let start = Instant::now();
        let res = self.discover_inner(goal, opts).await;
        let latency_ms = start.elapsed().as_secs_f64() * 1e3;
        let (outcome, detail) = match &res {
            Ok(d) => (
                "ok",
                d.tools
                    .iter()
                    .map(|h| h.id.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            Err(e) => ("error", format!("{e:#}")),
        };
        self.store.log_call(
            session,
            "discover",
            &truncate(goal, 200),
            outcome,
            latency_ms,
            &detail,
        );
        res.map(|mut d| {
            d.latency_ms = latency_ms;
            d
        })
    }

    async fn discover_inner(&self, goal: &str, opts: DiscoverOpts) -> anyhow::Result<Discovery> {
        ensure!(
            !goal.trim().is_empty(),
            "goal must describe what you are trying to do"
        );
        let q = tokens(goal);
        let kw = self.bm25.scores(&q);
        let sem = match opts.mode {
            Mode::Keyword => None,
            Mode::Semantic | Mode::Hybrid => {
                let vectors = self.prepare_semantic().await?;
                let qv = self.embedder.embed_query(goal).await?;
                Some(
                    vectors
                        .iter()
                        .map(|v| semantic::dot(v, &qv))
                        .collect::<Vec<f32>>(),
                )
            }
        };
        let scores = match (opts.mode, &sem) {
            (Mode::Keyword, _) | (_, None) => kw.clone(),
            (Mode::Semantic, Some(s)) => s.iter().map(|x| x.max(1e-6)).collect(),
            (Mode::Hybrid, Some(s)) => rrf(&[&kw, s]),
        };
        let observed = self.observed.read().unwrap_or_else(|e| e.into_inner());
        let picks = discover::select(
            &self.tools,
            &scores,
            &self.graph,
            &observed,
            opts.limit,
            opts.deps,
        );
        let tools = picks
            .iter()
            .map(|p| {
                let t = &self.tools[p.idx];
                let why = match (&p.edge, p.role) {
                    (Some(e), _) if e.to == t.id => match e.count {
                        Some(n) => format!(
                            "called after {} in {n} past session(s), passing `{}`",
                            e.from, e.via
                        ),
                        None => format!("can use `{}` returned by {}", e.via, e.from),
                    },
                    (Some(e), _) => format!("provides `{}` that {} requires", e.via, e.to),
                    (None, _) => self.match_reason(p.idx, &q, &kw, sem.as_deref()),
                };
                Hit {
                    id: t.id.clone(),
                    summary: t.summary(),
                    role: p.role,
                    why,
                }
            })
            .collect();
        Ok(Discovery {
            tools,
            edges: picks.into_iter().filter_map(|p| p.edge).collect(),
            catalog_size: self.tools.len(),
            latency_ms: 0.0,
        })
    }

    fn match_reason(&self, i: usize, q: &[String], kw: &[f32], sem: Option<&[f32]>) -> String {
        let words = self.bm25.matched(i, q);
        let mut parts = vec![];
        if kw[i] > 0.0 {
            parts.push(format!("matches {}", words.join(", ")));
        }
        if let Some(s) = sem {
            parts.push(format!("semantic similarity {:.2}", s[i]));
        }
        parts.join("; ")
    }

    pub fn inspect(&self, session: &str, id: &str) -> anyhow::Result<Inspection<'_>> {
        let start = Instant::now();
        let res = match self.tool(id) {
            Some(tool) => Ok(Inspection {
                tool,
                provenance: self.store.provenance(id)?,
                permission: self.policy.check(tool),
            }),
            None => Err(anyhow::anyhow!(
                "unknown tool id `{id}`; use discover_tools to find tool ids"
            )),
        };
        let outcome = if res.is_ok() { "ok" } else { "unknown" };
        self.store.log_call(
            session,
            "inspect",
            id,
            outcome,
            start.elapsed().as_secs_f64() * 1e3,
            "",
        );
        res
    }

    pub async fn invoke(&self, session: &str, id: &str, args: Value) -> Invocation {
        let start = Instant::now();
        let keys = args
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
            .unwrap_or_default();
        let inv = self.invoke_inner(session, id, args).await;
        let (outcome, detail) = match &inv {
            Invocation::Executed(r) if r.is_error == Some(true) => ("tool_error", ""),
            Invocation::Executed(_) => ("ok", ""),
            // Only the gateway's own messages: validation errors quote argument values and
            // upstream errors can echo anything.
            Invocation::Rejected {
                outcome: o @ ("denied" | "unknown"),
                message,
            } => (*o, message.as_str()),
            Invocation::Rejected { outcome, .. } => (*outcome, ""),
        };
        // Argument names only: values may carry secrets or personal data.
        let detail = format!("args=[{keys}] {detail}");
        self.store.log_call(
            session,
            "invoke",
            id,
            outcome,
            start.elapsed().as_secs_f64() * 1e3,
            detail.trim_end(),
        );
        inv
    }

    async fn invoke_inner(&self, session: &str, id: &str, args: Value) -> Invocation {
        let reject = |outcome, message: String| Invocation::Rejected { outcome, message };
        let Some(tool) = self.tool(id) else {
            return reject(
                "unknown",
                format!("unknown tool id `{id}`; use discover_tools to find tool ids"),
            );
        };
        if let Verdict::Deny { rule } = self.policy.check(tool) {
            return reject(
                "denied",
                format!(
                    "not executed: gateway policy denies {id} ({rule}). Finding a tool does not grant permission to run it."
                ),
            );
        }
        let args = if args.is_null() { json!({}) } else { args };
        if let Err(errors) = check_args(&tool.input_schema, &args) {
            return reject(
                "invalid",
                format!("invalid arguments for {id}: {}", errors.join("; ")),
            );
        }
        let up = match self.pool.get(&tool.server).await {
            Ok(up) => up,
            Err(e) => {
                return reject(
                    "failed",
                    format!("server {} unavailable: {e:#}", tool.server),
                );
            }
        };
        let Value::Object(map) = args.clone() else {
            unreachable!("validated as object")
        };
        match up.call(&tool.name, map).await {
            Err(e) => reject("failed", format!("{e:#}")),
            Ok(res) => {
                if res.is_error != Some(true) {
                    self.learn(session, id, &args, &result_value(&res));
                }
                Invocation::Executed(res)
            }
        }
    }

    /// Records which earlier results this successful call consumed, then remembers its output.
    fn learn(&self, session: &str, id: &str, args: &Value, result: &Value) {
        if !self.learn {
            return;
        }
        let mut memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        for (src, via) in memory.attribute(session, id, args) {
            if let Err(e) = self.store.add_observed_edge(&src, id, &via) {
                tracing::error!(error = %e, "could not persist observed edge");
            }
            self.observed
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .add(&src, id, &via);
        }
        memory.remember(session, id, result);
    }
}

/// The JSON a tool returned: structured content if present, else its text parsed as JSON.
pub fn result_value(res: &CallToolResult) -> Value {
    if let Some(v) = &res.structured_content {
        return v.clone();
    }
    let text: String = res
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect();
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

/// Reciprocal rank fusion over score vectors (0 = not retrieved by that ranker).
fn rrf(lists: &[&[f32]]) -> Vec<f32> {
    const K: f32 = 60.0;
    let n = lists.first().map_or(0, |l| l.len());
    let mut out = vec![0.0; n];
    for l in lists {
        let mut idx: Vec<usize> = (0..n).filter(|i| l[*i] > 0.0).collect();
        idx.sort_by(|a, b| l[*b].total_cmp(&l[*a]));
        for (rank, i) in idx.into_iter().enumerate() {
            out[i] += 1.0 / (K + rank as f32 + 1.0);
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_rewards_agreement() {
        let a = [3.0, 2.0, 0.0];
        let b = [0.1, 0.9, 0.5];
        let r = rrf(&[&a, &b]);
        assert!(r[1] > r[0] && r[0] > r[2], "{r:?}");
        assert!(rrf(&[]).is_empty());
    }

    #[tokio::test]
    async fn audit_log_never_stores_argument_values() {
        let dir = tempfile::tempdir().unwrap();
        let schema = json!({"type": "object", "properties": {"limit": {"type": "integer"}}});
        let tool = rmcp::model::Tool::new(
            "list",
            "List things.",
            std::sync::Arc::new(schema.as_object().unwrap().clone()),
        );
        let policy = Policy {
            default: crate::policy::Default_::Allow,
            ..Policy::default()
        };
        let lens = Lens::new(
            vec![ToolRecord::from_mcp("s", &tool)],
            Pool::new(BTreeMap::new(), 1),
            vec![],
            policy,
            Store::open(&dir.path().join("audit.db")).unwrap(),
        );
        let inv = lens
            .invoke("t", "s/list", json!({"limit": "4111111111111111"}))
            .await;
        assert!(matches!(
            inv,
            Invocation::Rejected {
                outcome: "invalid",
                ..
            }
        ));
        let calls = lens.store.calls(Some("t"), 10).unwrap();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].detail.contains("args=[limit]"),
            "{}",
            calls[0].detail
        );
        assert!(!calls[0].detail.contains("4111"), "{}", calls[0].detail);
    }
}
