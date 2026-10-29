//! Deterministic distractor tools. Each noise server covers five business domains, and some
//! domains are deliberate near-misses for the incident tasks (CI build logs, pod logs,
//! release windows, incidents, feature-flag rollouts). Server names look like real servers,
//! so an agent cannot spot the distractors by name.

use std::sync::Arc;

use serde_json::{Value, json};

use super::{Args, Fixture, tool};

const DOMAINS: &[(&str, &[&str])] = &[
    ("crm", &["lead", "contact", "opportunity", "account"]),
    (
        "hr",
        &["employee", "timesheet", "leave_request", "payroll_run"],
    ),
    (
        "billing",
        &["invoice", "subscription", "refund", "service_charge"],
    ),
    ("support", &["ticket", "macro", "sla_policy", "escalation"]),
    ("ci", &["pipeline", "build", "build_log", "artifact"]),
    ("k8s", &["pod", "pod_log", "node", "namespace"]),
    (
        "release",
        &["release", "release_note", "deploy_window", "change_request"],
    ),
    (
        "incident",
        &["incident", "postmortem", "status_page", "runbook"],
    ),
    (
        "feature_flags",
        &["flag", "flag_rule", "experiment", "rollout"],
    ),
    (
        "marketing",
        &["campaign", "segment", "email_template", "landing_page"],
    ),
    (
        "iot",
        &["sensor", "device", "firmware_update", "telemetry_reading"],
    ),
    (
        "analytics",
        &["dashboard", "report", "metric_definition", "funnel"],
    ),
    (
        "storage",
        &["bucket", "object", "lifecycle_rule", "snapshot"],
    ),
    ("dns", &["zone", "record", "certificate", "health_check"]),
    (
        "security",
        &["vulnerability", "scan", "secret", "access_review"],
    ),
    (
        "finance",
        &["ledger_entry", "budget", "expense", "forecast"],
    ),
    (
        "inventory",
        &["sku", "warehouse", "stock_level", "purchase_order"],
    ),
    ("docs", &["page", "space", "comment", "revision"]),
    ("chat", &["channel", "message", "thread", "reaction"]),
    (
        "legal",
        &["contract", "clause", "signature_request", "matter"],
    ),
];

/// Domains whose records reference services, so their tools compete for `service_id`.
const SERVICE_SCOPED: &[&str] = &[
    "billing",
    "ci",
    "k8s",
    "release",
    "incident",
    "feature_flags",
];

const VERBS: &[&str] = &[
    "list", "get", "search", "create", "update", "delete", "export", "archive",
];

const SERVER_NAMES: &[&str] = &[
    "bizops",
    "people-ops",
    "revenue",
    "helpdesk",
    "platform-ci",
    "cluster",
    "release-mgmt",
    "incident-hub",
    "growth",
    "edge",
    "insights",
    "workplace",
];

const REGIONS: &[&str] = &[
    "",
    " (EU region)",
    " (APAC region)",
    " (sandbox)",
    " (legacy v1 API)",
];

pub fn server_name(seed: usize) -> String {
    let base = SERVER_NAMES[seed % SERVER_NAMES.len()];
    match seed / SERVER_NAMES.len() {
        0 => base.to_string(),
        n => format!("{base}-{}", n + 1),
    }
}

fn plural(o: &str) -> String {
    if o.ends_with('s') || o.ends_with("ch") {
        format!("{o}es")
    } else if let Some(stem) = o
        .strip_suffix('y')
        .filter(|s| !s.ends_with(['a', 'e', 'o', 'u']))
    {
        format!("{stem}ies")
    } else {
        format!("{o}s")
    }
}

fn title(d: &str) -> String {
    d.split('_')
        .map(|w| w[..1].to_uppercase() + &w[1..])
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn tools(count: usize, seed: usize) -> Vec<rmcp::model::Tool> {
    let region = REGIONS[seed % REGIONS.len()];
    let mut out = vec![];
    for k in 0..5 {
        let (domain, objects) = DOMAINS[(seed * 5 + k) % DOMAINS.len()];
        let scoped = SERVICE_SCOPED.contains(&domain);
        let dt = title(domain);
        if k == 0 {
            out.push(tool("search", &format!("Search across all {dt} records{region}."), json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}), None, false));
            out.push(tool(
                "get_status",
                &format!("Get the {dt} system status and open incidents{region}."),
                json!({"type": "object", "properties": {}}),
                None,
                false,
            ));
        }
        for (oi, object) in objects.iter().enumerate() {
            let id_field = format!("{object}_id");
            let mut item = json!({"type": "object", "properties": {id_field.clone(): {"type": "string"}, "name": {"type": "string"}, "status": {"type": "string"}}});
            if scoped {
                item["properties"]["service_id"] = json!({"type": "string"});
            }
            for v in 0..5 {
                let verb = VERBS[(oi + v + seed) % VERBS.len()];
                let (desc, props, required, output, destructive): (
                    String,
                    Value,
                    Vec<&str>,
                    Option<Value>,
                    bool,
                ) = match verb {
                    "list" => (
                        format!(
                            "List {} in {dt}{region}, filtered by status{}.",
                            plural(object).replace('_', " "),
                            if scoped { " or service" } else { "" }
                        ),
                        if scoped {
                            json!({"status": {"type": "string"}, "service_id": {"type": "string"}})
                        } else {
                            json!({"status": {"type": "string"}})
                        },
                        vec![],
                        Some(
                            json!({"type": "object", "properties": {"items": {"type": "array", "items": item.clone()}}}),
                        ),
                        false,
                    ),
                    "get" => (
                        format!(
                            "Get one {} from {dt}{region} by id.",
                            object.replace('_', " ")
                        ),
                        json!({id_field.clone(): {"type": "string"}}),
                        vec![id_field.as_str()],
                        Some(item.clone()),
                        false,
                    ),
                    "search" => (
                        format!(
                            "Search {} in {dt}{region} by text.",
                            plural(object).replace('_', " ")
                        ),
                        json!({"query": {"type": "string"}}),
                        vec!["query"],
                        Some(
                            json!({"type": "object", "properties": {"items": {"type": "array", "items": item.clone()}}}),
                        ),
                        false,
                    ),
                    "create" => (
                        format!("Create a {} in {dt}{region}.", object.replace('_', " ")),
                        json!({"name": {"type": "string"}}),
                        vec!["name"],
                        Some(item.clone()),
                        false,
                    ),
                    "update" => (
                        format!("Update a {} in {dt}{region}.", object.replace('_', " ")),
                        json!({id_field.clone(): {"type": "string"}, "fields": {"type": "object"}}),
                        vec![id_field.as_str()],
                        None,
                        false,
                    ),
                    "delete" => (
                        format!(
                            "Permanently delete a {} from {dt}{region}.",
                            object.replace('_', " ")
                        ),
                        json!({id_field.clone(): {"type": "string"}}),
                        vec![id_field.as_str()],
                        None,
                        true,
                    ),
                    "export" => (
                        format!(
                            "Export {} from {dt}{region} as CSV.",
                            plural(object).replace('_', " ")
                        ),
                        json!({}),
                        vec![],
                        None,
                        false,
                    ),
                    _ => (
                        format!("Archive a {} in {dt}{region}.", object.replace('_', " ")),
                        json!({id_field.clone(): {"type": "string"}}),
                        vec![id_field.as_str()],
                        None,
                        false,
                    ),
                };
                let name = format!(
                    "{verb}_{}",
                    if verb == "list" || verb == "search" || verb == "export" {
                        plural(object)
                    } else {
                        object.to_string()
                    }
                );
                let schema = json!({"type": "object", "properties": props, "required": required});
                out.push(tool(&name, &desc, schema, output, destructive));
            }
        }
    }
    out.truncate(count);
    out
}

pub fn server(count: usize, seed: usize) -> Fixture {
    let handler = |name: &str, a: &Args| -> Result<Value, String> {
        let verb = name.split('_').next().unwrap_or("");
        Ok(match verb {
            "list" | "search" | "export" => json!({"items": []}),
            "get" => {
                let id = a.values().find_map(Value::as_str).unwrap_or("unknown");
                return Err(format!("no record with id {id:?}"));
            }
            _ => json!({"ok": true}),
        })
    };
    Fixture {
        tools: tools(count, seed),
        handler: Arc::new(handler),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn deterministic_unique_and_sized() {
        let a: Vec<String> = tools(100, 3).iter().map(|t| t.name.to_string()).collect();
        let b: Vec<String> = tools(100, 3).iter().map(|t| t.name.to_string()).collect();
        assert_eq!(a, b);
        assert_eq!(a.len(), 100);
        assert_eq!(
            a.iter().collect::<HashSet<_>>().len(),
            100,
            "names unique within a server"
        );
        assert_eq!(tools(12, 0).len(), 12);
        assert!(tools(0, 0).is_empty());
    }

    #[test]
    fn server_names_unique_for_a_thousand_tools() {
        let names: HashSet<String> = (0..20).map(server_name).collect();
        assert_eq!(names.len(), 20);
        assert!(!names.iter().any(|n| n.contains("noise")));
    }
}
