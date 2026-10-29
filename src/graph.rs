//! Tool dependency graph from two sources:
//! - schema: tool A's output schema has a field that tool B takes as input;
//! - observed: B was called successfully with a value A returned earlier in the same session.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;
use serde_json::Value;

use crate::catalog::ToolRecord;

/// Field names too generic to imply that one tool feeds another.
const GENERIC: &[&str] = &[
    "id",
    "name",
    "query",
    "limit",
    "offset",
    "cursor",
    "page",
    "page_size",
    "since",
    "until",
    "from",
    "text",
    "input",
    "value",
    "type",
    "status",
    "level",
    "state",
    "format",
    "ref",
    "path",
    "title",
    "message",
    "count",
    "sort",
    "order",
    "window_minutes",
    "description",
    "created_at",
    "updated_at",
];

pub fn linkable(field: &str) -> bool {
    field.len() >= 4 && !GENERIC.contains(&field)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Schema,
    Observed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub via: String,
    pub kind: EdgeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

/// A field shared by at most this many servers can link tools across servers: a few servers
/// sharing `deployment_id` are one organisation's suite; many sharing `region` are coincidence.
pub const MAX_CROSS_SERVER_SPREAD: usize = 3;

pub struct SchemaGraph {
    producers: HashMap<String, Vec<usize>>,
    consumers: HashMap<String, Vec<usize>>,
    outputs: Vec<Vec<String>>,
    resolvers: HashSet<(usize, String)>,
    /// Number of distinct servers using each field (as input or output).
    spread: HashMap<String, usize>,
}

impl SchemaGraph {
    pub fn build(tools: &[ToolRecord]) -> Self {
        let mut producers: HashMap<String, Vec<usize>> = HashMap::new();
        let mut consumers: HashMap<String, Vec<usize>> = HashMap::new();
        let mut outputs = vec![];
        let mut resolvers = HashSet::new();
        let mut servers_by_field: HashMap<String, HashSet<&str>> = HashMap::new();
        for (i, t) in tools.iter().enumerate() {
            let out: Vec<String> = t
                .output_fields()
                .into_iter()
                .filter(|f| linkable(f))
                .collect();
            for f in &out {
                servers_by_field
                    .entry(f.clone())
                    .or_default()
                    .insert(&t.server);
                producers.entry(f.clone()).or_default().push(i);
                if t.resolves(f) {
                    resolvers.insert((i, f.clone()));
                }
            }
            for (p, _, _) in t.params() {
                if linkable(p) {
                    servers_by_field
                        .entry(p.to_string())
                        .or_default()
                        .insert(&t.server);
                    consumers.entry(p.to_string()).or_default().push(i);
                }
            }
            outputs.push(out);
        }
        let spread = servers_by_field
            .into_iter()
            .map(|(f, s)| (f, s.len()))
            .collect();
        Self {
            producers,
            consumers,
            outputs,
            resolvers,
            spread,
        }
    }

    /// Whether `field` may link tools of different servers (see `MAX_CROSS_SERVER_SPREAD`).
    pub fn crosses_servers(&self, field: &str) -> bool {
        self.spread
            .get(field)
            .is_some_and(|n| *n <= MAX_CROSS_SERVER_SPREAD)
    }

    /// Tool `i` maps human-readable names to `field` (see `ToolRecord::resolves`).
    pub fn resolves(&self, i: usize, field: &str) -> bool {
        self.resolvers.contains(&(i, field.to_string()))
    }

    pub fn producers(&self, field: &str) -> &[usize] {
        self.producers.get(field).map_or(&[], Vec::as_slice)
    }

    pub fn consumers(&self, field: &str) -> &[usize] {
        self.consumers.get(field).map_or(&[], Vec::as_slice)
    }

    /// Linkable output fields of tool `i`.
    pub fn outputs(&self, i: usize) -> &[String] {
        &self.outputs[i]
    }
}

#[derive(Default)]
pub struct Observed {
    out: HashMap<String, Vec<(String, String, u32)>>,
}

impl Observed {
    pub fn from_rows(rows: Vec<(String, String, String, u32)>) -> Self {
        let mut o = Self::default();
        for (src, dst, via, n) in rows {
            o.out.entry(src).or_default().push((dst, via, n));
        }
        o
    }

    pub fn add(&mut self, src: &str, dst: &str, via: &str) {
        let list = self.out.entry(src.to_string()).or_default();
        match list.iter_mut().find(|(d, v, _)| d == dst && v == via) {
            Some(e) => e.2 += 1,
            None => list.push((dst.to_string(), via.to_string(), 1)),
        }
    }

    /// (dst, via, count), most frequent first.
    pub fn successors(&self, src: &str) -> Vec<(String, String, u32)> {
        let mut v = self.out.get(src).cloned().unwrap_or_default();
        v.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        v
    }
}

const MAX_SESSIONS: usize = 1024;
const MAX_CALLS_PER_SESSION: usize = 32;
const MAX_VALUES_PER_CALL: usize = 512;

/// Recent successful results per session, bounded on every axis, used to attribute
/// argument values to the call that produced them.
#[derive(Default)]
pub struct SessionMemory {
    sessions: HashMap<String, VecDeque<(String, HashSet<String>)>>,
    order: VecDeque<String>,
}

impl SessionMemory {
    pub fn remember(&mut self, session: &str, tool_id: &str, result: &Value) {
        let mut values = HashSet::new();
        collect_values(result, &mut values);
        if values.is_empty() {
            return;
        }
        if !self.sessions.contains_key(session) {
            if self.order.len() == MAX_SESSIONS
                && let Some(old) = self.order.pop_front()
            {
                self.sessions.remove(&old);
            }
            self.order.push_back(session.to_string());
        }
        let calls = self.sessions.entry(session.to_string()).or_default();
        if calls.len() == MAX_CALLS_PER_SESSION {
            calls.pop_front();
        }
        calls.push_back((tool_id.to_string(), values));
    }

    /// For each top-level argument whose value an earlier call returned: (producer tool, arg name).
    pub fn attribute(&self, session: &str, tool_id: &str, args: &Value) -> Vec<(String, String)> {
        let (Some(calls), Some(args)) = (self.sessions.get(session), args.as_object()) else {
            return vec![];
        };
        let mut found = vec![];
        for (name, v) in args {
            let Some(s) = scalar(v) else { continue };
            if let Some((src, _)) = calls
                .iter()
                .rev()
                .find(|(id, vals)| id != tool_id && vals.contains(&s))
            {
                found.push((src.clone(), name.clone()));
            }
        }
        found
    }
}

/// Identifier-like scalars only: short strings and small numbers are too likely to collide.
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if s.len() >= 4 && s.len() <= 200 => Some(s.clone()),
        Value::Number(n) if n.as_u64().is_some_and(|n| n >= 1000) => Some(n.to_string()),
        _ => None,
    }
}

fn collect_values(v: &Value, out: &mut HashSet<String>) {
    if out.len() >= MAX_VALUES_PER_CALL {
        return;
    }
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect_values(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_values(x, out)),
        other => {
            if let Some(s) = scalar(other) {
                out.insert(s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn attributes_argument_to_most_recent_producer() {
        let mut m = SessionMemory::default();
        m.remember(
            "s",
            "deploy/list",
            &json!({"deployments": [{"deployment_id": "dep-1", "commit_sha": "9f3c2ab"}]}),
        );
        m.remember("s", "vcs/search", &json!({"commits": [{"sha": "9f3c2ab"}]}));
        let got = m.attribute(
            "s",
            "vcs/get_commit",
            &json!({"commit_sha": "9f3c2ab", "limit": 5}),
        );
        assert_eq!(got, vec![("vcs/search".into(), "commit_sha".into())]);
        assert!(
            m.attribute("other", "x", &json!({"commit_sha": "9f3c2ab"}))
                .is_empty()
        );
        assert!(m.attribute("s", "x", &json!("not an object")).is_empty());
    }

    #[test]
    fn session_memory_is_bounded() {
        let mut m = SessionMemory::default();
        for i in 0..MAX_SESSIONS + 10 {
            m.remember(&format!("s{i}"), "t", &json!({"v": "value"}));
        }
        assert_eq!(m.sessions.len(), MAX_SESSIONS);
        for _ in 0..MAX_CALLS_PER_SESSION + 5 {
            m.remember("s5000", "t", &json!({"v": "value"}));
        }
        assert_eq!(m.sessions["s5000"].len(), MAX_CALLS_PER_SESSION);
    }

    #[test]
    fn observed_successors_sorted_by_count() {
        let mut o = Observed::default();
        o.add("a", "b", "x");
        o.add("a", "c", "y");
        o.add("a", "c", "y");
        assert_eq!(o.successors("a")[0], ("c".into(), "y".into(), 2));
        assert!(o.successors("zzz").is_empty());
    }
}
