//! Connections to upstream MCP servers over stdio.
//!
//! - Indexing: servers are started with bounded parallelism; each tool list is cached in SQLite
//!   under a hash of the server's config, so a restart does not need to start any server.
//! - Lazy mode: a server starts on the first call to one of its tools; at most `max_live` run at
//!   once and the least recently used is stopped first. Eager mode keeps every server running.
//! - Calls: per-call timeout (dropping the request sends MCP cancellation) and a bound on calls
//!   in flight per server. Each server's stderr is drained into the log, bounded per line.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, anyhow};
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, Tool},
    service::RunningService,
    transport::TokioChildProcess,
};
use tokio::{
    io::AsyncReadExt,
    process::ChildStderr,
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
};

use crate::{
    catalog::fnv1a,
    config::{Config, ServerConfig},
    store::Store,
};

/// Generous: the first start of an `npx`/`uvx` server in a fresh sandbox downloads the package.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_IN_FLIGHT: usize = 8;
const CONNECT_PARALLELISM: usize = 6;
const STDERR_TAIL_LINES: usize = 25;
const STDERR_MAX_LINE: usize = 400;

type Tail = Arc<Mutex<VecDeque<String>>>;

pub struct Upstream {
    pub name: String,
    client: RunningService<RoleClient, ()>,
    permits: Semaphore,
}

impl Upstream {
    async fn connect(name: &str, cfg: &ServerConfig) -> anyhow::Result<(Self, Vec<Tool>)> {
        let program = program(cfg)?;
        let mut cmd = tokio::process::Command::new(&program);
        // Upstream servers are third-party code: they get the system basics plus their own
        // configured env, never the gateway's secrets (API keys, tokens).
        cmd.args(&cfg.args)
            .env_clear()
            .envs(inherited_env(std::env::vars()))
            .envs(&cfg.env);
        let (transport, stderr) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning {}", command_line(cfg)))?;
        let tail: Tail = Arc::default();
        let drainer = stderr.map(|e| tokio::spawn(drain(name.to_string(), e, tail.clone())));
        let started = async {
            let client = ().serve(transport).await.context("initializing")?;
            let tools = client.list_all_tools().await.context("listing tools")?;
            anyhow::Ok((client, tools))
        };
        match started.await {
            Ok((client, tools)) => Ok((
                Self {
                    name: name.to_string(),
                    client,
                    permits: Semaphore::new(MAX_IN_FLIGHT),
                },
                tools,
            )),
            Err(e) => Err(with_stderr(e, drainer, &tail).await),
        }
    }

    pub async fn call(
        &self,
        tool: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> anyhow::Result<CallToolResult> {
        // Bounded queue: callers wait for a permit, but not longer than the call itself may take.
        let _permit = tokio::time::timeout(CALL_TIMEOUT, self.permits.acquire())
            .await
            .map_err(|_| {
                anyhow!(
                    "{} is saturated ({MAX_IN_FLIGHT} calls in flight)",
                    self.name
                )
            })??;
        let params = CallToolRequestParams::new(tool.to_string()).with_arguments(args);
        tokio::time::timeout(CALL_TIMEOUT, self.client.call_tool(params))
            .await
            .map_err(|_| {
                anyhow!(
                    "{}/{tool} timed out after {}s (cancelled)",
                    self.name,
                    CALL_TIMEOUT.as_secs()
                )
            })?
            .with_context(|| format!("{}/{tool} failed", self.name))
    }
}

/// Appends the server's last stderr lines: a failed start is otherwise undiagnosable.
async fn with_stderr(
    e: anyhow::Error,
    drainer: Option<JoinHandle<()>>,
    tail: &Tail,
) -> anyhow::Error {
    if let Some(d) = drainer {
        // The process has usually exited; give the drainer a moment to reach EOF.
        let _ = tokio::time::timeout(Duration::from_secs(2), d).await;
    }
    let lines: Vec<String> = tail
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .cloned()
        .collect();
    if lines.is_empty() {
        e
    } else {
        anyhow!("{e:#} | stderr: {}", lines.join(" / "))
    }
}

/// Reads stderr in chunks so a line without a newline cannot grow without bound.
async fn drain(server: String, mut stderr: ChildStderr, tail: Tail) {
    let mut buf = [0u8; 8192];
    let mut line: Vec<u8> = vec![];
    let emit = |line: &mut Vec<u8>| {
        let text = String::from_utf8_lossy(line).trim_end().to_string();
        line.clear();
        if text.is_empty() {
            return;
        }
        tracing::debug!(server = %server, "{text}");
        let mut t = tail.lock().unwrap_or_else(|p| p.into_inner());
        if t.len() == STDERR_TAIL_LINES {
            t.pop_front();
        }
        t.push_back(text);
    };
    while let Ok(n) = stderr.read(&mut buf).await {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' {
                emit(&mut line);
            } else if line.len() < STDERR_MAX_LINE {
                line.push(b);
            }
        }
    }
    emit(&mut line);
}

fn program(cfg: &ServerConfig) -> anyhow::Result<PathBuf> {
    if cfg.command == "@self" {
        return std::env::current_exe().context("locating toollens executable");
    }
    Ok(resolve(
        cfg.command.as_ref(),
        &std::env::var_os("PATH").unwrap_or_default(),
        &std::env::var("PATHEXT").unwrap_or_default(),
    ))
}

/// The program's file name only. This reaches the model (`inspect_tool` provenance) and error
/// messages; args routinely carry credentials (`--api-key`, connection strings, auth headers) and
/// paths carry user names.
pub fn command_line(cfg: &ServerConfig) -> String {
    Path::new(&cfg.command)
        .file_name()
        .map_or_else(|| cfg.command.clone(), |n| n.to_string_lossy().into_owned())
}

/// Identity of a server's launch config: a change means its tool list must be re-indexed.
pub fn config_hash(cfg: &ServerConfig) -> String {
    format!(
        "{:016x}",
        fnv1a(serde_json::to_string(cfg).unwrap_or_default().as_bytes())
    )
}

/// Running servers, started on demand.
pub struct Pool {
    configs: BTreeMap<String, ServerConfig>,
    /// Least recently used first.
    live: Mutex<Vec<(String, Arc<Upstream>)>>,
    starting: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    max_live: usize,
}

impl Pool {
    pub fn new(configs: BTreeMap<String, ServerConfig>, max_live: usize) -> Self {
        Self {
            configs,
            live: Mutex::default(),
            starting: Default::default(),
            max_live: max_live.max(1),
        }
    }

    pub fn live_count(&self) -> usize {
        self.live.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    fn take_live(&self, server: &str) -> Option<Arc<Upstream>> {
        let mut live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        let i = live.iter().position(|(n, _)| n == server)?;
        let entry = live.remove(i);
        // A server that exited (crash, OOM) is dropped here, so the caller starts a fresh one.
        if entry.1.client.is_transport_closed() {
            tracing::warn!(server, "upstream server exited; restarting it");
            return None;
        }
        let up = entry.1.clone();
        live.push(entry);
        Some(up)
    }

    fn insert(&self, server: &str, up: Arc<Upstream>) {
        let mut live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        live.retain(|(n, _)| n != server);
        live.push((server.to_string(), up));
        while live.len() > self.max_live {
            // Dropping the last reference stops the server; calls in flight keep their own Arc.
            let (evicted, _) = live.remove(0);
            tracing::info!(server = %evicted, "stopping least recently used server");
        }
    }

    pub async fn get(&self, server: &str) -> anyhow::Result<Arc<Upstream>> {
        if let Some(up) = self.take_live(server) {
            return Ok(up);
        }
        let cfg = self
            .configs
            .get(server)
            .ok_or_else(|| anyhow!("server {server} is not configured"))?;
        // One start per server at a time: concurrent first calls must not spawn duplicates.
        let gate = self
            .starting
            .lock()
            .await
            .entry(server.to_string())
            .or_default()
            .clone();
        let _starting = gate.lock().await;
        if let Some(up) = self.take_live(server) {
            return Ok(up);
        }
        let (up, _) = tokio::time::timeout(CONNECT_TIMEOUT, Upstream::connect(server, cfg))
            .await
            .map_err(|_| {
                anyhow!(
                    "starting {server} timed out after {}s",
                    CONNECT_TIMEOUT.as_secs()
                )
            })??;
        let up = Arc::new(up);
        self.insert(server, up.clone());
        Ok(up)
    }
}

pub struct Index {
    /// (server, tools), sorted by server.
    pub servers: Vec<(String, Vec<Tool>)>,
    pub failed: Vec<(String, String)>,
    pub from_cache: usize,
}

/// Lists every server's tools: from the cache when lazy and the config is unchanged, otherwise
/// by starting the server (at most `CONNECT_PARALLELISM` at once). Failures are reported, not
/// fatal. Eager mode keeps the started servers in the pool; lazy mode stops them after listing.
pub async fn index(cfg: &Config, store: &Store, pool: &Pool) -> Index {
    let mut out = Index {
        servers: vec![],
        failed: vec![],
        from_cache: 0,
    };
    let mut to_start = vec![];
    for (name, sc) in &cfg.mcp_servers {
        let cached = if cfg.lazy && !cfg.reindex {
            store.cached_tools(name, &config_hash(sc))
        } else {
            Ok(None)
        };
        match cached {
            Ok(Some(tools)) => {
                out.from_cache += 1;
                out.servers.push((name.clone(), tools));
            }
            Ok(None) => to_start.push((name.clone(), sc.clone())),
            Err(e) => {
                tracing::warn!(server = %name, error = %e, "tool cache unreadable; re-indexing");
                to_start.push((name.clone(), sc.clone()));
            }
        }
    }
    let gate = Arc::new(Semaphore::new(CONNECT_PARALLELISM));
    let mut set = JoinSet::new();
    for (name, sc) in to_start {
        let gate = gate.clone();
        set.spawn(async move {
            let _slot = gate.acquire_owned().await.expect("semaphore open");
            let res = tokio::time::timeout(CONNECT_TIMEOUT, Upstream::connect(&name, &sc))
                .await
                .unwrap_or_else(|_| Err(anyhow!("timed out after {}s", CONNECT_TIMEOUT.as_secs())));
            (name, sc, res)
        });
    }
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((name, sc, Ok((up, tools)))) => {
                if let Err(e) = store.put_tools(&name, &config_hash(&sc), &tools) {
                    tracing::error!(server = %name, error = %e, "could not cache tool list");
                }
                if !cfg.lazy {
                    pool.insert(&name, Arc::new(up));
                }
                out.servers.push((name, tools));
            }
            Ok((name, _, Err(e))) => {
                tracing::warn!(server = %name, error = %format!("{e:#}"), "upstream unavailable");
                out.failed.push((name, format!("{e:#}")));
            }
            Err(e) => out.failed.push(("<task>".into(), e.to_string())),
        }
    }
    out.servers.sort_by(|a, b| a.0.cmp(&b.0));
    out.failed.sort();
    out
}

/// Variables an upstream server inherits: the MCP TypeScript SDK's default set (what Claude
/// Desktop passes) plus a few non-secret Windows basics needed to run `.cmd` launchers.
const INHERITED_ENV: &[&str] = if cfg!(windows) {
    &[
        "APPDATA",
        "HOMEDRIVE",
        "HOMEPATH",
        "LOCALAPPDATA",
        "PATH",
        "PROCESSOR_ARCHITECTURE",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "TEMP",
        "USERNAME",
        "USERPROFILE",
        "PROGRAMFILES",
        "PATHEXT",
        "COMSPEC",
        "WINDIR",
        "TMP",
    ]
} else {
    &["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER"]
};

/// Windows env names are case-insensitive (`Path`), so match them that way there.
fn inherited_env(vars: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    vars.filter(|(k, _)| {
        INHERITED_ENV.iter().any(|allowed| {
            if cfg!(windows) {
                allowed.eq_ignore_ascii_case(k)
            } else {
                allowed == k
            }
        })
    })
    .collect()
}

/// On Windows a bare name only resolves to `.exe`, but MCP configs routinely say `npx` or `uvx`
/// (`npx.cmd`). Search PATH x PATHEXT the way a shell does; std escapes arguments for `.cmd`/`.bat`.
/// Elsewhere, and for paths or names with an extension, the program is used as given.
fn resolve(program: &Path, path: &OsStr, pathext: &str) -> PathBuf {
    if !cfg!(windows) || program.components().count() > 1 || program.extension().is_some() {
        return program.to_path_buf();
    }
    let exts = if pathext.is_empty() {
        ".COM;.EXE;.BAT;.CMD"
    } else {
        pathext
    };
    std::env::split_paths(path)
        .flat_map(|dir| {
            exts.split(';')
                .filter_map(|e| e.strip_prefix('.'))
                .map(move |e| dir.join(program).with_extension(e))
        })
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| program.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_env_drops_secrets_and_keeps_system_basics() {
        let parent = [
            ("ANTHROPIC_API_KEY", "sk-secret"),
            ("GITHUB_TOKEN", "t"),
            ("Path", "C:/bin"),
            ("PATH", "/bin"),
            ("HOME", "/h"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let kept: Vec<String> = inherited_env(parent.into_iter())
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert!(
            !kept
                .iter()
                .any(|k| k == "ANTHROPIC_API_KEY" || k == "GITHUB_TOKEN")
        );
        if cfg!(windows) {
            assert_eq!(kept, ["Path", "PATH"], "case-insensitive on Windows");
        } else {
            assert_eq!(kept, ["PATH", "HOME"]);
        }
    }

    #[test]
    fn resolves_bare_names_through_pathext_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fakenpx.cmd"), "@echo off").unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        let got = resolve(Path::new("fakenpx"), &path, ".EXE;.CMD");
        if cfg!(windows) {
            // Windows file names are case-insensitive; PATHEXT supplies the upper-case form.
            assert_eq!(got, dir.path().join("fakenpx.CMD"));
            assert!(got.is_file());
        } else {
            assert_eq!(got, Path::new("fakenpx"));
        }
        // Paths, names with an extension, and unknown names are used as given.
        assert_eq!(
            resolve(Path::new("fakenpx.cmd"), &path, ".CMD"),
            Path::new("fakenpx.cmd")
        );
        assert_eq!(
            resolve(Path::new("C:/x/y"), &path, ".CMD"),
            Path::new("C:/x/y")
        );
        assert_eq!(
            resolve(Path::new("missing"), &path, ".CMD"),
            Path::new("missing")
        );
    }

    #[test]
    fn config_hash_changes_with_launch_config() {
        let a: ServerConfig =
            serde_json::from_str(r#"{"command":"npx","args":["-y","x@1"]}"#).unwrap();
        let b: ServerConfig =
            serde_json::from_str(r#"{"command":"npx","args":["-y","x@2"]}"#).unwrap();
        assert_eq!(config_hash(&a), config_hash(&a.clone()));
        assert_ne!(config_hash(&a), config_hash(&b));
    }

    #[test]
    fn command_line_never_carries_args_or_directories() {
        let cfg: ServerConfig = serde_json::from_str(
            r#"{"command":"C:/Users/someone/bin/pg-mcp.exe","args":["--url","postgres://u:hunter2@db/x"]}"#,
        )
        .unwrap();
        assert_eq!(command_line(&cfg), "pg-mcp.exe");
    }

    #[tokio::test]
    async fn a_server_that_exited_is_dropped_not_reused() {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let (server, client) = tokio::join!(
            crate::fixtures::infra::deploy().serve(tokio::io::split(server_io)),
            ().serve(tokio::io::split(client_io)),
        );
        let up = Arc::new(Upstream {
            name: "deploy".into(),
            client: client.unwrap(),
            permits: Semaphore::new(MAX_IN_FLIGHT),
        });
        let pool = Pool::new(BTreeMap::new(), 2);
        pool.insert("deploy", up.clone());
        assert!(pool.take_live("deploy").is_some());

        server.unwrap().cancel().await.unwrap();
        for _ in 0..200 {
            if up.client.is_transport_closed() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            up.client.is_transport_closed(),
            "the server side was shut down"
        );
        assert!(
            pool.take_live("deploy").is_none(),
            "a dead server must not be reused"
        );
        assert_eq!(pool.live_count(), 0);
    }
}
