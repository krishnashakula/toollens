//! Turns per-tool relevance scores into a small, dependency-complete tool set.
//!
//! The top matches become seeds. For each seed, in rank order, we add: tools that produce
//! its required inputs (so the agent can actually call it), tools seen after it in past
//! sessions, and tools that consume its outputs (the likely next step). Remaining budget
//! is filled with the next-best matches.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{
    catalog::ToolRecord,
    graph::{Edge, EdgeKind, Observed, SchemaGraph, linkable},
    text::tokens,
};

pub const MAX_LIMIT: usize = 10;
const SEEDS: usize = 3;
const CONSUMERS_PER_SEED: usize = 2;
const OBSERVED_PER_SEED: usize = 2;
/// Direct matches allowed per tool family (same server, same name minus its verb), so five
/// CRUD variants of one near-miss object cannot crowd out everything else.
const MATCHES_PER_FAMILY: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Match,
    Dependency,
}

#[derive(Debug, Clone)]
pub struct Pick {
    pub idx: usize,
    pub role: Role,
    pub edge: Option<Edge>,
}

pub fn ranked(scores: &[f32], tools: &[ToolRecord]) -> Vec<usize> {
    let mut r: Vec<usize> = (0..scores.len()).filter(|i| scores[*i] > 0.0).collect();
    r.sort_by(|a, b| {
        scores[*b]
            .total_cmp(&scores[*a])
            .then_with(|| tools[*a].id.cmp(&tools[*b].id))
    });
    r
}

pub fn select(
    tools: &[ToolRecord],
    scores: &[f32],
    graph: &SchemaGraph,
    observed: &Observed,
    limit: usize,
    deps: bool,
) -> Vec<Pick> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let mut families: HashMap<String, usize> = HashMap::new();
    let ranked: Vec<usize> = ranked(scores, tools)
        .into_iter()
        .filter(|i| {
            let n = families.entry(family(&tools[*i])).or_default();
            *n += 1;
            *n <= MATCHES_PER_FAMILY
        })
        .collect();
    // Half the slots are reserved for direct matches: expansion adds context, it must not
    // push the best matches out (on real catalogs it did, before this cap).
    let reserved = ranked.len().min(limit.div_ceil(2));
    let mut s = Selection {
        picks: vec![],
        seen: HashSet::new(),
        limit,
        max_deps: limit - reserved,
        deps: 0,
    };
    if deps {
        for &seed in ranked.iter().take(SEEDS) {
            s.push(seed, Role::Match, None);
            add_producers(&mut s, tools, scores, graph, seed, 2);
            add_observed(&mut s, tools, observed, seed);
            add_consumers(&mut s, tools, scores, graph, seed);
        }
    }
    for &i in &ranked {
        s.push(i, Role::Match, None);
    }
    s.picks
}

/// `server` + tool name without its leading verb: list_change_requests and
/// archive_change_request are one family.
fn family(t: &ToolRecord) -> String {
    let words = tokens(&t.name);
    let rest = if words.len() > 1 {
        &words[1..]
    } else {
        &words[..]
    };
    format!("{}/{}", t.server, rest.join(" "))
}

struct Selection {
    picks: Vec<Pick>,
    seen: HashSet<usize>,
    limit: usize,
    /// Dependencies may fill at most this many slots; the rest stay for direct matches.
    max_deps: usize,
    deps: usize,
}

impl Selection {
    fn push(&mut self, idx: usize, role: Role, edge: Option<Edge>) -> bool {
        let deps_full = role == Role::Dependency && self.deps >= self.max_deps;
        if self.picks.len() >= self.limit || deps_full || self.seen.contains(&idx) {
            return false;
        }
        self.seen.insert(idx);
        if role == Role::Dependency {
            self.deps += 1;
        }
        self.picks.push(Pick { idx, role, edge });
        true
    }
}

fn required_links(t: &ToolRecord) -> usize {
    t.params()
        .iter()
        .filter(|(p, _, req)| *req && linkable(p))
        .count()
}

/// Producer for one of `tool`'s inputs. Other servers qualify only through specific fields
/// (`SchemaGraph::crosses_servers`). Prefer resolvers (name -> id lookups, the usual entry
/// point), then the same server (ids line up within a server), then tools that need fewer
/// linked inputs themselves (avoids circular picks like get_deployment <-> list_deployments),
/// then relevance, then id for determinism.
fn best_producer(
    g: &SchemaGraph,
    field: &str,
    tool: usize,
    tools: &[ToolRecord],
    scores: &[f32],
) -> Option<usize> {
    g.producers(field)
        .iter()
        .copied()
        .filter(|c| {
            *c != tool && (tools[*c].server == tools[tool].server || g.crosses_servers(field))
        })
        .max_by(|a, b| {
            let same = |c: &usize| tools[*c].server == tools[tool].server;
            g.resolves(*a, field)
                .cmp(&g.resolves(*b, field))
                .then_with(|| same(a).cmp(&same(b)))
                .then_with(|| required_links(&tools[*b]).cmp(&required_links(&tools[*a])))
                .then_with(|| scores[*a].total_cmp(&scores[*b]))
                .then_with(|| tools[*b].id.cmp(&tools[*a].id))
        })
}

fn add_producers(
    s: &mut Selection,
    tools: &[ToolRecord],
    scores: &[f32],
    g: &SchemaGraph,
    tool: usize,
    depth: u8,
) {
    if depth == 0 {
        return;
    }
    for (param, _, required) in tools[tool].params() {
        if !required || !linkable(param) {
            continue;
        }
        let Some(p) = best_producer(g, param, tool, tools, scores) else {
            continue;
        };
        if s.push(
            p,
            Role::Dependency,
            Some(edge(tools, p, tool, param, EdgeKind::Schema, None)),
        ) {
            add_producers(s, tools, scores, g, p, depth - 1);
        }
    }
}

/// Likely next steps: tools that accept the seed's outputs. A field shared by many tools
/// (`service_id`) is weak evidence; a specific one (`commit_sha`) is strong, so each shared
/// field counts 1/(number of tools that accept it). Destructive tools are never suggested
/// speculatively; they only appear when the goal itself matches them. Tools of other servers
/// qualify only through specific fields (`SchemaGraph::crosses_servers`).
fn add_consumers(
    s: &mut Selection,
    tools: &[ToolRecord],
    scores: &[f32],
    g: &SchemaGraph,
    seed: usize,
) {
    let mut cands: Vec<(usize, f32, &str)> = vec![];
    for f in g.outputs(seed) {
        let consumers = g.consumers(f);
        let w = 1.0 / consumers.len() as f32;
        for &c in consumers {
            let cross = tools[c].server != tools[seed].server;
            if c == seed || tools[c].destructive_hint() || (cross && !g.crosses_servers(f)) {
                continue;
            }
            match cands.iter_mut().find(|x| x.0 == c) {
                Some(x) => x.1 += w,
                None => cands.push((c, w, f.as_str())),
            }
        }
    }
    cands.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| scores[b.0].total_cmp(&scores[a.0]))
            .then_with(|| tools[a.0].id.cmp(&tools[b.0].id))
    });
    let mut added = 0;
    for (c, _, via) in cands {
        if added == CONSUMERS_PER_SEED {
            break;
        }
        if s.push(
            c,
            Role::Dependency,
            Some(edge(tools, seed, c, via, EdgeKind::Schema, None)),
        ) {
            added += 1;
        }
    }
}

fn add_observed(s: &mut Selection, tools: &[ToolRecord], observed: &Observed, seed: usize) {
    let mut added = 0;
    for (dst, via, n) in observed.successors(&tools[seed].id) {
        if added == OBSERVED_PER_SEED {
            break;
        }
        let Some(d) = tools.iter().position(|t| t.id == dst) else {
            continue;
        };
        if s.push(
            d,
            Role::Dependency,
            Some(edge(tools, seed, d, &via, EdgeKind::Observed, Some(n))),
        ) {
            added += 1;
        }
    }
}

fn edge(
    tools: &[ToolRecord],
    from: usize,
    to: usize,
    via: &str,
    kind: EdgeKind,
    count: Option<u32>,
) -> Edge {
    Edge {
        from: tools[from].id.clone(),
        to: tools[to].id.clone(),
        via: via.into(),
        kind,
        count,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::*;

    fn tool(id: &str, inputs: &[(&str, bool)], outputs: &[&str]) -> ToolRecord {
        let (server, name) = id.split_once('/').unwrap();
        let props: serde_json::Map<String, Value> = inputs
            .iter()
            .map(|(n, _)| (n.to_string(), json!({"type": "string"})))
            .collect();
        let req: Vec<&str> = inputs.iter().filter(|(_, r)| *r).map(|(n, _)| *n).collect();
        let schema = json!({"type": "object", "properties": props, "required": req});
        let out: serde_json::Map<String, Value> = outputs
            .iter()
            .map(|n| (n.to_string(), json!({"type": "string"})))
            .collect();
        let t = rmcp::model::Tool::new(
            name.to_string(),
            "d",
            Arc::new(schema.as_object().unwrap().clone()),
        )
        .with_raw_output_schema(Arc::new(
            json!({"type": "object", "properties": out})
                .as_object()
                .unwrap()
                .clone(),
        ));
        ToolRecord::from_mcp(server, &t)
    }

    fn catalog() -> Vec<ToolRecord> {
        vec![
            tool(
                "deploy/list_deployments",
                &[("service_id", true)],
                &["deployment_id", "commit_sha", "service_id"],
            ),
            tool("deploy/search", &[("query", true)], &["service_id"]),
            tool(
                "obs/get_logs",
                &[("service_id", true), ("deployment_id", false)],
                &[],
            ),
            tool("vcs/get_commit", &[("commit_sha", true)], &["author"]),
            tool("billing/list_services", &[], &["service_id"]),
            tool("crm/send_email", &[("to", true)], &[]),
            tool(
                "deploy/get_deployment",
                &[("deployment_id", true)],
                &["service_id", "commit_sha"],
            ),
            tool("obs/get_status", &[("service_id", true)], &[]),
        ]
    }

    fn ids(tools: &[ToolRecord], picks: &[Pick]) -> Vec<String> {
        picks.iter().map(|p| tools[p.idx].id.clone()).collect()
    }

    #[test]
    fn expands_seed_with_producer_and_consumers() {
        let tools = catalog();
        let g = SchemaGraph::build(&tools);
        let mut scores = [0.0; 8];
        scores[0] = 1.0;
        let picks = select(&tools, &scores, &g, &Observed::default(), 8, true);
        // Producer: same-server entry point deploy/search, not get_deployment (needs a deployment_id
        // first) or billing. Consumers: commit_sha (1 consumer) outweighs hub field service_id.
        assert_eq!(
            ids(&tools, &picks),
            [
                "deploy/list_deployments",
                "deploy/search",
                "vcs/get_commit",
                "obs/get_logs"
            ]
        );
        assert_eq!(picks[1].edge.as_ref().unwrap().via, "service_id");
        assert_eq!(picks[1].role, Role::Dependency);
    }

    #[test]
    fn destructive_tools_are_not_suggested_as_next_steps() {
        let mut tools = catalog();
        tools.push(tool("deploy/rollback", &[("deployment_id", true)], &[]));
        let i = tools.len() - 1;
        tools[i].annotations = Some(serde_json::json!({"destructiveHint": true}));
        let g = SchemaGraph::build(&tools);
        let mut scores = vec![0.0; tools.len()];
        scores[0] = 1.0;
        assert!(
            !ids(
                &tools,
                &select(&tools, &scores, &g, &Observed::default(), 10, true)
            )
            .contains(&"deploy/rollback".to_string())
        );
        scores[i] = 0.5;
        assert!(
            ids(
                &tools,
                &select(&tools, &scores, &g, &Observed::default(), 10, true)
            )
            .contains(&"deploy/rollback".to_string())
        );
    }

    #[test]
    fn without_deps_returns_only_matches_and_respects_limit() {
        let tools = catalog();
        let g = SchemaGraph::build(&tools);
        let scores = [0.9, 0.0, 0.5, 0.0, 0.0, 0.1, 0.0, 0.0];
        let ids: Vec<usize> = select(&tools, &scores, &g, &Observed::default(), 2, false)
            .iter()
            .map(|p| p.idx)
            .collect();
        assert_eq!(ids, [0, 2]);
        assert!(select(&tools, &[0.0; 8], &g, &Observed::default(), 5, true).is_empty());
        assert_eq!(
            select(&tools, &[1.0; 8], &g, &Observed::default(), 100, true).len(),
            8
        );
    }

    #[test]
    fn prefers_name_resolvers_over_incidental_producers() {
        let tools = vec![
            tool("obs/query_metric", &[("service_id", true)], &[]),
            tool("obs/search", &[("query", true)], &["service_id", "message"]),
            tool("deploy/search", &[("query", true)], &["service_id", "name"]),
        ];
        let g = SchemaGraph::build(&tools);
        let picks = select(&tools, &[1.0, 0.0, 0.0], &g, &Observed::default(), 8, true);
        assert_eq!(ids(&tools, &picks), ["obs/query_metric", "deploy/search"]);
    }

    #[test]
    fn caps_matches_per_tool_family() {
        let tools: Vec<ToolRecord> = [
            "list_change_requests",
            "archive_change_request",
            "delete_change_request",
            "get_pull_request",
        ]
        .iter()
        .map(|n| tool(&format!("x/{n}"), &[], &[]))
        .collect();
        let g = SchemaGraph::build(&tools);
        let picks = select(
            &tools,
            &[0.9, 0.8, 0.7, 0.1],
            &g,
            &Observed::default(),
            8,
            false,
        );
        assert_eq!(
            ids(&tools, &picks),
            [
                "x/list_change_requests",
                "x/archive_change_request",
                "x/get_pull_request"
            ]
        );
    }

    #[test]
    fn dependencies_never_take_the_reserved_match_slots() {
        let mut tools = catalog();
        // Ten extra direct matches, each with its own same-server producer.
        for i in 0..10 {
            tools.push(tool(
                &format!("s{i}/get_thing"),
                &[(&format!("thing{i}_id"), true)],
                &[],
            ));
            tools.push(tool(
                &format!("s{i}/find_things"),
                &[],
                &[&format!("thing{i}_id")],
            ));
        }
        let g = SchemaGraph::build(&tools);
        let scores: Vec<f32> = tools
            .iter()
            .map(|t| if t.name == "get_thing" { 1.0 } else { 0.0 })
            .collect();
        let picks = select(&tools, &scores, &g, &Observed::default(), 8, true);
        assert_eq!(picks.len(), 8);
        assert!(
            picks.iter().filter(|p| p.role == Role::Match).count() >= 4,
            "half the slots stay direct matches"
        );
    }

    #[test]
    fn widely_shared_fields_do_not_link_servers() {
        // `region` is produced or consumed by five servers: coincidence, not a data flow.
        let mut tools = vec![tool("a/get_cluster", &[("region", true)], &[])];
        for s in ["b", "c", "d", "e"] {
            tools.push(tool(&format!("{s}/list_regions"), &[], &["region"]));
        }
        let g = SchemaGraph::build(&tools);
        let mut scores = vec![0.0; tools.len()];
        scores[0] = 1.0;
        let many = ids(
            &tools,
            &select(&tools, &scores, &g, &Observed::default(), 8, true),
        );
        assert_eq!(many, ["a/get_cluster"]);
        // Three servers sharing a field are treated as one suite and do link.
        let few: Vec<ToolRecord> = tools[..3].to_vec();
        let g = SchemaGraph::build(&few);
        let linked = ids(
            &few,
            &select(&few, &[1.0, 0.0, 0.0], &g, &Observed::default(), 8, true),
        );
        assert_eq!(linked.len(), 2, "{linked:?}");
    }

    #[test]
    fn observed_edges_add_learned_next_steps() {
        let tools = catalog();
        let g = SchemaGraph::build(&tools);
        let mut o = Observed::default();
        o.add("vcs/get_commit", "crm/send_email", "to");
        let scores = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let picks = select(&tools, &scores, &g, &o, 8, true);
        let email = picks
            .iter()
            .find(|p| tools[p.idx].id == "crm/send_email")
            .expect("observed successor");
        assert_eq!(email.edge.as_ref().unwrap().kind, EdgeKind::Observed);
    }
}
