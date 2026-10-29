//! SQLite persistence: tool provenance, embedding cache, audit log of every gateway call,
//! and dependency edges learned from successful sequences.

use std::{path::Path, sync::Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::catalog::ToolRecord;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS tools (
    id TEXT PRIMARY KEY,
    server TEXT NOT NULL,
    command TEXT NOT NULL,
    schema_hash TEXT NOT NULL,
    first_hash TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS embeddings (
    key TEXT PRIMARY KEY,
    vector BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS calls (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    session TEXT NOT NULL,
    kind TEXT NOT NULL,
    target TEXT NOT NULL,
    outcome TEXT NOT NULL,
    latency_ms REAL NOT NULL,
    detail TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS observed_edges (
    src TEXT NOT NULL,
    dst TEXT NOT NULL,
    via TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (src, dst, via)
);
CREATE TABLE IF NOT EXISTS server_tools (
    server TEXT PRIMARY KEY,
    config_hash TEXT NOT NULL,
    tools TEXT NOT NULL,
    indexed_at TEXT NOT NULL
);
";

const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";

/// shortcut: one connection behind a mutex; every statement is a sub-millisecond point
/// write. Move to a connection pool + spawn_blocking if the audit log shows contention.
pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Provenance {
    pub server: String,
    pub command: String,
    pub first_seen: String,
    pub last_seen: String,
    pub schema_hash: String,
    /// True when the tool's definition differs from the one first indexed: the server
    /// changed name, description or schema after the operator last reviewed it.
    pub changed_since_first_seen: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallLog {
    pub ts: String,
    pub session: String,
    pub kind: String,
    pub target: String,
    pub outcome: String,
    pub latency_ms: f64,
    pub detail: String,
}

impl Store {
    /// `:memory:` gives a private in-memory database.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves SQLite itself consistent; keep serving.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn record_tool(&self, tool: &ToolRecord, command: &str) -> anyhow::Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO tools (id, server, command, schema_hash, first_hash, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?4, {NOW}, {NOW})
                 ON CONFLICT(id) DO UPDATE SET command = ?3, schema_hash = ?4, last_seen = {NOW}"
            ),
            params![tool.id, tool.server, command, tool.schema_hash],
        )?;
        Ok(())
    }

    pub fn provenance(&self, id: &str) -> anyhow::Result<Option<Provenance>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT server, command, first_seen, last_seen, schema_hash, first_hash FROM tools WHERE id = ?1",
                [id],
                |r| {
                    let hash: String = r.get(4)?;
                    let first: String = r.get(5)?;
                    Ok(Provenance {
                        server: r.get(0)?,
                        command: r.get(1)?,
                        first_seen: r.get(2)?,
                        last_seen: r.get(3)?,
                        changed_since_first_seen: hash != first,
                        schema_hash: hash,
                    })
                },
            )
            .optional()?)
    }

    pub fn embedding(&self, key: &str) -> anyhow::Result<Option<Vec<f32>>> {
        let blob: Option<Vec<u8>> = self
            .conn()
            .query_row("SELECT vector FROM embeddings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(blob.map(|b| {
            b.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        }))
    }

    pub fn put_embedding(&self, key: &str, v: &[f32]) -> anyhow::Result<()> {
        let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.conn().execute(
            "INSERT OR REPLACE INTO embeddings (key, vector) VALUES (?1, ?2)",
            params![key, blob],
        )?;
        Ok(())
    }

    pub fn log_call(
        &self,
        session: &str,
        kind: &str,
        target: &str,
        outcome: &str,
        latency_ms: f64,
        detail: &str,
    ) {
        let res = self.conn().execute(
            &format!(
                "INSERT INTO calls (ts, session, kind, target, outcome, latency_ms, detail)
                 VALUES ({NOW}, ?1, ?2, ?3, ?4, ?5, ?6)"
            ),
            params![session, kind, target, outcome, latency_ms, detail],
        );
        // The audit write must not fail the user's call, but it must not vanish either.
        if let Err(e) = res {
            tracing::error!(error = %e, kind, target, "audit log write failed");
        }
    }

    pub fn calls(&self, session: Option<&str>, limit: usize) -> anyhow::Result<Vec<CallLog>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT ts, session, kind, target, outcome, latency_ms, detail FROM calls
             WHERE ?1 IS NULL OR session = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![session, limit as i64], |r| {
            Ok(CallLog {
                ts: r.get(0)?,
                session: r.get(1)?,
                kind: r.get(2)?,
                target: r.get(3)?,
                outcome: r.get(4)?,
                latency_ms: r.get(5)?,
                detail: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_observed_edge(&self, src: &str, dst: &str, via: &str) -> anyhow::Result<()> {
        self.conn().execute(
            "INSERT INTO observed_edges (src, dst, via, count) VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(src, dst, via) DO UPDATE SET count = count + 1",
            params![src, dst, via],
        )?;
        Ok(())
    }

    pub fn observed_edges(&self) -> anyhow::Result<Vec<(String, String, String, u32)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT src, dst, via, count FROM observed_edges")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

impl Store {
    /// Tool list indexed for `server` under exactly this config, if any.
    pub fn cached_tools(
        &self,
        server: &str,
        config_hash: &str,
    ) -> anyhow::Result<Option<Vec<rmcp::model::Tool>>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT tools FROM server_tools WHERE server = ?1 AND config_hash = ?2",
                params![server, config_hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
    }

    pub fn put_tools(
        &self,
        server: &str,
        config_hash: &str,
        tools: &[rmcp::model::Tool],
    ) -> anyhow::Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO server_tools (server, config_hash, tools, indexed_at) VALUES (?1, ?2, ?3, {NOW})
                 ON CONFLICT(server) DO UPDATE SET config_hash = ?2, tools = ?3, indexed_at = {NOW}"
            ),
            params![server, config_hash, serde_json::to_string(tools)?],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(desc: &str) -> ToolRecord {
        let t = rmcp::model::Tool::new(
            "t",
            desc.to_string(),
            std::sync::Arc::new(json!({"type":"object"}).as_object().unwrap().clone()),
        );
        ToolRecord::from_mcp("s", &t)
    }

    #[test]
    fn provenance_flags_changed_definitions() {
        let s = Store::in_memory().unwrap();
        s.record_tool(&tool("v1"), "cmd").unwrap();
        s.record_tool(&tool("v1"), "cmd").unwrap();
        assert!(
            !s.provenance("s/t")
                .unwrap()
                .unwrap()
                .changed_since_first_seen
        );
        s.record_tool(&tool("v2 - now also deletes things"), "cmd")
            .unwrap();
        assert!(
            s.provenance("s/t")
                .unwrap()
                .unwrap()
                .changed_since_first_seen
        );
        assert!(s.provenance("s/missing").unwrap().is_none());
    }

    #[test]
    fn tool_cache_is_keyed_by_config_hash() {
        let s = Store::in_memory().unwrap();
        let t = rmcp::model::Tool::new(
            "t",
            "d",
            std::sync::Arc::new(json!({"type":"object"}).as_object().unwrap().clone()),
        );
        s.put_tools("srv", "h1", std::slice::from_ref(&t)).unwrap();
        assert_eq!(s.cached_tools("srv", "h1").unwrap().unwrap()[0].name, "t");
        assert!(
            s.cached_tools("srv", "h2").unwrap().is_none(),
            "changed config means re-index"
        );
        assert!(s.cached_tools("other", "h1").unwrap().is_none());
    }

    #[test]
    fn embeddings_roundtrip_and_edges_count() {
        let s = Store::in_memory().unwrap();
        s.put_embedding("k", &[1.5, -2.0]).unwrap();
        assert_eq!(s.embedding("k").unwrap(), Some(vec![1.5, -2.0]));
        assert_eq!(s.embedding("nope").unwrap(), None);
        s.add_observed_edge("a", "b", "x").unwrap();
        s.add_observed_edge("a", "b", "x").unwrap();
        assert_eq!(
            s.observed_edges().unwrap(),
            vec![("a".into(), "b".into(), "x".into(), 2)]
        );
    }
}
