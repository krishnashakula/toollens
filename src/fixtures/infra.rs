//! One coherent (fictional) incident across three servers. Production checkout-api deploy
//! dep-4821 failed because commit 9f3c2ab dropped STRIPE_WEBHOOK_SECRET from the env template.
//! Every server has a `search` and a `get_status` that mean different things.

use std::sync::{Arc, LazyLock};

use serde_json::{Value, json};

use super::{Args, Fixture, arg, tool};

static DATA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "services": [
            {"service_id": "svc-checkout", "name": "checkout-api", "team": "payments", "repo": "acme/checkout-api"},
            {"service_id": "svc-search", "name": "search-api", "team": "discovery", "repo": "acme/search-api"},
            {"service_id": "svc-payworker", "name": "payments-worker", "team": "payments", "repo": "acme/payments-worker"},
            {"service_id": "svc-auth", "name": "auth-service", "team": "identity", "repo": "acme/auth-service"}
        ],
        "deployments": [
            {"deployment_id": "dep-4821", "service_id": "svc-checkout", "version": "v2.14.0", "commit_sha": "9f3c2ab", "previous_commit_sha": "41d7e0c", "status": "failed", "environment": "production", "started_at": "2026-10-24T14:02:11Z", "triggered_by": "dana.k", "failure_reason": "health check failed: 3/3 pods in CrashLoopBackOff; traffic kept on v2.13.2"},
            {"deployment_id": "dep-4790", "service_id": "svc-checkout", "version": "v2.13.2", "commit_sha": "41d7e0c", "previous_commit_sha": "7be19d0", "status": "succeeded", "environment": "production", "started_at": "2026-10-22T09:15:40Z", "triggered_by": "lee.m"},
            {"deployment_id": "dep-4733", "service_id": "svc-checkout", "version": "v2.13.1", "commit_sha": "7be19d0", "previous_commit_sha": "02aa9c4", "status": "succeeded", "environment": "production", "started_at": "2026-10-19T10:01:02Z", "triggered_by": "dana.k"},
            {"deployment_id": "dep-4818", "service_id": "svc-search", "version": "v5.2.0", "commit_sha": "c0ffee1", "previous_commit_sha": "b16b00b", "status": "succeeded", "environment": "production", "started_at": "2026-10-24T11:00:00Z", "triggered_by": "jo.w"},
            {"deployment_id": "dep-4702", "service_id": "svc-search", "version": "v5.1.3", "commit_sha": "b16b00b", "previous_commit_sha": "a11ce55", "status": "succeeded", "environment": "production", "started_at": "2026-10-17T10:20:00Z", "triggered_by": "ana.b"},
            {"deployment_id": "dep-4815", "service_id": "svc-payworker", "version": "v1.9.3", "commit_sha": "5ad0b3e", "previous_commit_sha": "3e1f2d0", "status": "rolled_back", "environment": "production", "started_at": "2026-10-23T16:40:27Z", "triggered_by": "sam.r", "failure_reason": "automatic rollback: error rate above 5% SLO for 10 minutes"},
            {"deployment_id": "dep-4810", "service_id": "svc-auth", "version": "v3.0.1", "commit_sha": "e1e10f0", "previous_commit_sha": "d00dfee", "status": "succeeded", "environment": "production", "started_at": "2026-10-23T08:30:00Z", "triggered_by": "kai.t"}
        ],
        "logs": [
            {"service_id": "svc-checkout", "deployment_id": "dep-4821", "ts": "2026-10-24T14:02:31Z", "level": "error", "message": "config: required env var STRIPE_WEBHOOK_SECRET is not set"},
            {"service_id": "svc-checkout", "deployment_id": "dep-4821", "ts": "2026-10-24T14:02:31Z", "level": "fatal", "message": "panic: failed to initialize payment webhooks: missing webhook secret"},
            {"service_id": "svc-checkout", "deployment_id": "dep-4821", "ts": "2026-10-24T14:02:45Z", "level": "warn", "message": "readiness probe failed: dial tcp :8080: connection refused"},
            {"service_id": "svc-checkout", "deployment_id": "dep-4790", "ts": "2026-10-24T14:03:10Z", "level": "info", "message": "serving traffic on v2.13.2"},
            {"service_id": "svc-payworker", "deployment_id": "dep-4815", "ts": "2026-10-23T16:44:02Z", "level": "error", "message": "ledger-service call timed out after 200ms (retry budget exhausted)"},
            {"service_id": "svc-payworker", "deployment_id": "dep-4815", "ts": "2026-10-23T16:44:05Z", "level": "warn", "message": "circuit breaker open for ledger-service"},
            {"service_id": "svc-search", "deployment_id": "dep-4818", "ts": "2026-10-24T11:02:00Z", "level": "info", "message": "index warmup complete in 41s"},
            {"service_id": "svc-auth", "deployment_id": "dep-4810", "ts": "2026-10-24T13:10:00Z", "level": "warn", "message": "token issuance p99 latency 820ms exceeds 500ms target"}
        ],
        "metrics": {
            "svc-checkout": {"error_rate": 0.31, "latency_p99": 240, "request_rate": 1840, "cpu": 41},
            "svc-search": {"error_rate": 0.42, "latency_p99": 95, "request_rate": 5210, "cpu": 63},
            "svc-payworker": {"error_rate": 0.12, "latency_p99": 310, "request_rate": 220, "cpu": 22},
            "svc-auth": {"error_rate": 0.05, "latency_p99": 820, "request_rate": 3100, "cpu": 58}
        },
        "alerts": [
            {"alert": "CheckoutDeployFailed", "service_id": "svc-checkout", "severity": "warning", "state": "active", "since": "2026-10-24T14:04:00Z"},
            {"alert": "AuthTokenLatencyHigh", "service_id": "svc-auth", "severity": "critical", "state": "active", "since": "2026-10-24T13:12:00Z"},
            {"alert": "PaymentsWorkerErrorRate", "service_id": "svc-payworker", "severity": "critical", "state": "resolved", "since": "2026-10-23T16:45:00Z"}
        ],
        "oncall": {
            "payments": {"primary": "priya.n", "secondary": "omar.f"},
            "discovery": {"primary": "jo.w", "secondary": "ana.b"},
            "identity": {"primary": "kai.t", "secondary": "lee.m"}
        },
        "commits": [
            {"commit_sha": "9f3c2ab", "repo": "acme/checkout-api", "author": "dana.k", "date": "2026-10-24T12:40:00Z", "message": "Load webhook secrets through new ConfigLoader", "pull_request": 1287, "files": ["src/config/loader.rs", "deploy/checkout-api.env.tmpl"], "diff": "deploy/checkout-api.env.tmpl: - STRIPE_WEBHOOK_SECRET=${STRIPE_WEBHOOK_SECRET}\nsrc/config/loader.rs: + secrets now read from ConfigLoader namespace \"payments\" (namespace not provisioned in production)"},
            {"commit_sha": "41d7e0c", "repo": "acme/checkout-api", "author": "lee.m", "date": "2026-10-21T16:02:00Z", "message": "Bump stripe-sdk to 12.3", "pull_request": 1279, "files": ["Cargo.toml", "Cargo.lock"], "diff": "Cargo.toml: - stripe-sdk = \"12.2\" + stripe-sdk = \"12.3\""},
            {"commit_sha": "7be19d0", "repo": "acme/checkout-api", "author": "dana.k", "date": "2026-10-18T11:20:00Z", "message": "Add idempotency keys to refund endpoint", "pull_request": 1265, "files": ["src/refunds.rs"], "diff": "src/refunds.rs: + idempotency_key header handling"},
            {"commit_sha": "5ad0b3e", "repo": "acme/payments-worker", "author": "sam.r", "date": "2026-10-23T15:55:00Z", "message": "Lower ledger client timeout from 2s to 200ms", "pull_request": 412, "files": ["src/ledger/client.rs"], "diff": "src/ledger/client.rs: - timeout: Duration::from_secs(2) + timeout: Duration::from_millis(200)"},
            {"commit_sha": "c0ffee1", "repo": "acme/search-api", "author": "jo.w", "date": "2026-10-24T09:30:00Z", "message": "Improve ranking for long queries", "pull_request": 733, "files": ["src/rank.rs"], "diff": "src/rank.rs: + length-normalized BM25"},
            {"commit_sha": "e1e10f0", "repo": "acme/auth-service", "author": "kai.t", "date": "2026-10-22T17:45:00Z", "message": "Rotate token signing keys", "pull_request": 208, "files": ["keys/rotation.rs"], "diff": "keys/rotation.rs: + rotate every 24h"}
        ],
        "pull_requests": [
            {"repo": "acme/checkout-api", "number": 1287, "title": "Move webhook secrets to ConfigLoader", "author": "dana.k", "state": "merged", "merged_at": "2026-10-24T12:41:00Z"},
            {"repo": "acme/checkout-api", "number": 1279, "title": "Bump stripe-sdk to 12.3", "author": "lee.m", "state": "merged", "merged_at": "2026-10-21T16:03:00Z"},
            {"repo": "acme/checkout-api", "number": 1291, "title": "Restore STRIPE_WEBHOOK_SECRET env mapping", "author": "priya.n", "state": "open", "merged_at": null},
            {"repo": "acme/payments-worker", "number": 412, "title": "Tighten ledger timeouts", "author": "sam.r", "state": "merged", "merged_at": "2026-10-23T15:56:00Z"}
        ],
        "ci": {
            "acme/checkout-api": {"branch": "main", "state": "passing", "last_run": "2026-10-24T12:52:00Z", "note": "unit and integration tests do not load production env templates"},
            "acme/search-api": {"branch": "main", "state": "passing", "last_run": "2026-10-24T09:41:00Z"},
            "acme/payments-worker": {"branch": "main", "state": "failing", "last_run": "2026-10-24T08:12:00Z", "note": "flaky ledger contract test"},
            "acme/auth-service": {"branch": "main", "state": "passing", "last_run": "2026-10-22T18:00:00Z"}
        }
    })
});

fn rows(table: &str) -> impl Iterator<Item = &'static Value> {
    DATA[table].as_array().into_iter().flatten()
}

fn find(table: &str, key: &str, value: &str) -> Option<&'static Value> {
    rows(table).find(|r| r[key] == value)
}

fn service(id: &str) -> Result<&'static Value, String> {
    find("services", "service_id", id).ok_or_else(|| {
        format!("unknown service_id {id:?}. service ids look like \"svc-...\"; resolve a service name with deploy/search")
    })
}

fn contains(hay: &Value, needle: &str) -> bool {
    hay.to_string()
        .to_lowercase()
        .contains(&needle.to_lowercase())
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

fn s(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

pub fn deploy() -> Fixture {
    let service_out = json!({"type": "object", "properties": {"services": {"type": "array", "items": {"type": "object", "properties": {
        "service_id": {"type": "string"}, "name": {"type": "string"}, "team": {"type": "string"}, "repo": {"type": "string"}}}}}});
    let deployment = json!({"type": "object", "properties": {
        "deployment_id": {"type": "string"}, "service_id": {"type": "string"}, "version": {"type": "string"},
        "commit_sha": {"type": "string"}, "previous_commit_sha": {"type": "string"}, "status": {"type": "string"},
        "environment": {"type": "string"}, "started_at": {"type": "string"}, "failure_reason": {"type": "string"}}});
    let tools = vec![
        tool(
            "search",
            "Search the service registry by service name, team or repository. Returns service ids, owning team and repo.",
            obj(
                json!({"query": s("Service name, team or repo fragment, e.g. \"checkout\".")}),
                &["query"],
            ),
            Some(service_out),
            false,
        ),
        tool(
            "get_status",
            "Get the rollout status of a service: the version currently serving traffic and the outcome of the latest deployment attempt.",
            obj(
                json!({"service_id": s("Service id such as svc-checkout.")}),
                &["service_id"],
            ),
            None,
            false,
        ),
        tool(
            "list_deployments",
            "List recent production deployments of a service, newest first, with version, commit and status.",
            obj(
                json!({"service_id": s("Service id such as svc-checkout."), "limit": {"type": "integer", "minimum": 1, "maximum": 20}}),
                &["service_id"],
            ),
            Some(
                json!({"type": "object", "properties": {"deployments": {"type": "array", "items": deployment.clone()}}}),
            ),
            false,
        ),
        tool(
            "get_deployment",
            "Get one deployment's details, including the failure reason when it failed or was rolled back.",
            obj(
                json!({"deployment_id": s("Deployment id such as dep-4821.")}),
                &["deployment_id"],
            ),
            Some(deployment),
            false,
        ),
        tool(
            "rollback",
            "Roll a service back to an earlier successful deployment. Changes production.",
            obj(
                json!({"service_id": s("Service id."), "deployment_id": s("Successful deployment to restore.")}),
                &["service_id", "deployment_id"],
            ),
            None,
            true,
        ),
        tool(
            "list_environments",
            "List deployment environments and their clusters.",
            obj(json!({}), &[]),
            None,
            false,
        ),
    ];
    let handler = |name: &str, a: &Args| -> Result<Value, String> {
        match name {
            "search" => {
                let q = arg(a, "query")?;
                Ok(
                    json!({"services": rows("services").filter(|r| contains(r, q)).collect::<Vec<_>>()}),
                )
            }
            "get_status" => {
                let id = arg(a, "service_id")?;
                service(id)?;
                let deps: Vec<_> = rows("deployments")
                    .filter(|d| d["service_id"] == id)
                    .collect();
                let serving = deps.iter().find(|d| d["status"] == "succeeded");
                Ok(
                    json!({"service_id": id, "serving": serving.map(|d| json!({"deployment_id": d["deployment_id"], "version": d["version"]})),
                          "latest_attempt": deps.first().map(|d| json!({"deployment_id": d["deployment_id"], "status": d["status"]}))}),
                )
            }
            "list_deployments" => {
                let id = arg(a, "service_id")?;
                service(id)?;
                let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(5) as usize;
                let deps: Vec<Value> = rows("deployments")
                    .filter(|d| d["service_id"] == id)
                    .take(limit)
                    .map(|d| {
                        let mut d = d.clone();
                        d.as_object_mut().map(|o| o.remove("failure_reason"));
                        d
                    })
                    .collect();
                Ok(json!({"deployments": deps}))
            }
            "get_deployment" => {
                let id = arg(a, "deployment_id")?;
                find("deployments", "deployment_id", id)
                    .cloned()
                    .ok_or_else(|| format!("unknown deployment_id {id:?}"))
            }
            "rollback" => {
                let (svc, dep) = (arg(a, "service_id")?, arg(a, "deployment_id")?);
                service(svc)?;
                Ok(json!({"service_id": svc, "rolled_back_to": dep, "status": "rollback started"}))
            }
            "list_environments" => Ok(
                json!({"environments": [{"name": "production", "cluster": "prod-use1"}, {"name": "staging", "cluster": "stg-use1"}]}),
            ),
            _ => Err(format!("unknown tool {name}")),
        }
    };
    Fixture {
        tools,
        handler: Arc::new(handler),
    }
}

pub fn observability() -> Fixture {
    let log_line = json!({"type": "object", "properties": {"ts": {"type": "string"}, "level": {"type": "string"}, "message": {"type": "string"},
        "service_id": {"type": "string"}, "deployment_id": {"type": "string"}}});
    let tools = vec![
        tool(
            "search",
            "Full-text search over log messages across all services.",
            obj(
                json!({"query": s("Text to look for in log messages."), "service_id": s("Optional service id filter.")}),
                &["query"],
            ),
            Some(
                json!({"type": "object", "properties": {"lines": {"type": "array", "items": log_line.clone()}}}),
            ),
            false,
        ),
        tool(
            "get_status",
            "Get a service's health: SLO compliance and whether it is degraded.",
            obj(
                json!({"service_id": s("Service id such as svc-checkout.")}),
                &["service_id"],
            ),
            None,
            false,
        ),
        tool(
            "get_logs",
            "Fetch log lines for a service, optionally for a single deployment and minimum level.",
            obj(
                json!({"service_id": s("Service id such as svc-checkout."), "deployment_id": s("Only lines from this deployment."),
                       "level": {"type": "string", "enum": ["debug", "info", "warn", "error", "fatal"]}, "limit": {"type": "integer", "minimum": 1, "maximum": 100}}),
                &["service_id"],
            ),
            Some(
                json!({"type": "object", "properties": {"lines": {"type": "array", "items": log_line}}}),
            ),
            false,
        ),
        tool(
            "query_metric",
            "Read a service metric over a time window: error_rate (%), latency_p99 (ms), request_rate (req/s) or cpu (%).",
            obj(
                json!({"service_id": s("Service id."), "metric": {"type": "string", "enum": ["error_rate", "latency_p99", "request_rate", "cpu"]},
                       "window_minutes": {"type": "integer", "minimum": 1, "maximum": 1440}}),
                &["service_id", "metric"],
            ),
            None,
            false,
        ),
        tool(
            "list_alerts",
            "List monitoring alerts, optionally for one service. Includes active and recently resolved alerts.",
            obj(json!({"service_id": s("Optional service id filter.")}), &[]),
            None,
            false,
        ),
        tool(
            "get_oncall",
            "Get the primary and secondary on-call engineers for a team.",
            obj(json!({"team": s("Team name, e.g. payments.")}), &["team"]),
            None,
            false,
        ),
    ];
    let levels = ["debug", "info", "warn", "error", "fatal"];
    let handler = move |name: &str, a: &Args| -> Result<Value, String> {
        match name {
            "search" => {
                let q = arg(a, "query")?;
                let svc = a.get("service_id").and_then(Value::as_str);
                let lines: Vec<_> = rows("logs")
                    .filter(|l| {
                        svc.is_none_or(|s| l["service_id"] == s) && contains(&l["message"], q)
                    })
                    .collect();
                Ok(json!({"lines": lines}))
            }
            "get_status" => {
                let id = arg(a, "service_id")?;
                service(id)?;
                let degraded =
                    rows("alerts").any(|al| al["service_id"] == id && al["state"] == "active");
                Ok(
                    json!({"service_id": id, "health": if degraded { "degraded" } else { "healthy" }, "slo_30d": if degraded { 99.71 } else { 99.97 }}),
                )
            }
            "get_logs" => {
                let id = arg(a, "service_id")?;
                service(id)?;
                let dep = a.get("deployment_id").and_then(Value::as_str);
                let min = a
                    .get("level")
                    .and_then(Value::as_str)
                    .and_then(|l| levels.iter().position(|x| *x == l))
                    .unwrap_or(0);
                let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
                let lines: Vec<_> = rows("logs")
                    .filter(|l| {
                        l["service_id"] == id && dep.is_none_or(|d| l["deployment_id"] == d)
                    })
                    .filter(|l| levels.iter().position(|x| l["level"] == *x).unwrap_or(0) >= min)
                    .take(limit)
                    .collect();
                Ok(json!({"lines": lines}))
            }
            "query_metric" => {
                let (id, metric) = (arg(a, "service_id")?, arg(a, "metric")?);
                service(id)?;
                let value = &DATA["metrics"][id][metric];
                if value.is_null() {
                    return Err(format!("unknown metric {metric:?}"));
                }
                let unit = match metric {
                    "error_rate" | "cpu" => "%",
                    "latency_p99" => "ms",
                    _ => "req/s",
                };
                Ok(
                    json!({"service_id": id, "metric": metric, "window_minutes": a.get("window_minutes").cloned().unwrap_or(json!(60)), "value": value, "unit": unit}),
                )
            }
            "list_alerts" => {
                let svc = a.get("service_id").and_then(Value::as_str);
                Ok(
                    json!({"alerts": rows("alerts").filter(|al| svc.is_none_or(|s| al["service_id"] == s)).collect::<Vec<_>>()}),
                )
            }
            "get_oncall" => {
                let team = arg(a, "team")?;
                let r = &DATA["oncall"][team];
                if r.is_null() {
                    Err(format!("unknown team {team:?}"))
                } else {
                    Ok(json!({"team": team, "oncall": r}))
                }
            }
            _ => Err(format!("unknown tool {name}")),
        }
    };
    Fixture {
        tools,
        handler: Arc::new(handler),
    }
}

pub fn vcs() -> Fixture {
    let commit = json!({"type": "object", "properties": {"commit_sha": {"type": "string"}, "repo": {"type": "string"}, "author": {"type": "string"},
        "message": {"type": "string"}, "pull_request": {"type": "integer"}, "files": {"type": "array", "items": {"type": "string"}}, "diff": {"type": "string"}}});
    let tools = vec![
        tool(
            "search",
            "Search commit messages, optionally within one repository.",
            obj(
                json!({"query": s("Words to find in commit messages."), "repo": s("Optional repo, e.g. acme/checkout-api.")}),
                &["query"],
            ),
            Some(
                json!({"type": "object", "properties": {"commits": {"type": "array", "items": commit.clone()}}}),
            ),
            false,
        ),
        tool(
            "get_status",
            "Get the CI status of a repository's main branch.",
            obj(
                json!({"repo": s("Repository, e.g. acme/checkout-api.")}),
                &["repo"],
            ),
            None,
            false,
        ),
        tool(
            "get_commit",
            "Get a commit's message, author, pull request, changed files and diff.",
            obj(
                json!({"commit_sha": s("Commit sha (7+ hex chars).")}),
                &["commit_sha"],
            ),
            Some(commit.clone()),
            false,
        ),
        tool(
            "compare_commits",
            "List the commits and changed files between two commits of a repository.",
            obj(
                json!({"repo": s("Repository."), "base_sha": s("Older commit."), "head_sha": s("Newer commit.")}),
                &["repo", "base_sha", "head_sha"],
            ),
            Some(
                json!({"type": "object", "properties": {"commits": {"type": "array", "items": commit}}}),
            ),
            false,
        ),
        tool(
            "list_pull_requests",
            "List pull requests of a repository with their state (open, merged, closed).",
            obj(
                json!({"repo": s("Repository."), "state": {"type": "string", "enum": ["open", "merged", "closed", "all"]}}),
                &["repo"],
            ),
            None,
            false,
        ),
        tool(
            "get_file",
            "Read a file from a repository at a given ref.",
            obj(
                json!({"repo": s("Repository."), "path": s("File path."), "ref": s("Branch or commit, default main.")}),
                &["repo", "path"],
            ),
            None,
            false,
        ),
    ];
    let handler = |name: &str, a: &Args| -> Result<Value, String> {
        let repo_known = |r: &str| {
            DATA["ci"].get(r).map(|_| ()).ok_or_else(|| {
                format!("unknown repo {r:?}; repos look like \"acme/<service-name>\"")
            })
        };
        match name {
            "search" => {
                let q = arg(a, "query")?;
                let repo = a.get("repo").and_then(Value::as_str);
                let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
                let hits: Vec<_> = rows("commits")
                    .filter(|c| repo.is_none_or(|r| c["repo"] == r))
                    .filter(|c| words.iter().any(|w| contains(&c["message"], w)))
                    .collect();
                Ok(json!({"commits": hits}))
            }
            "get_status" => {
                let r = arg(a, "repo")?;
                repo_known(r)?;
                Ok(json!({"repo": r, "ci": DATA["ci"][r]}))
            }
            "get_commit" => {
                let sha = arg(a, "commit_sha")?;
                rows("commits")
                    .find(|c| {
                        // `get`, not `[..7]`: byte 7 can fall inside a multi-byte character.
                        sha.get(..7).is_some_and(|p| {
                            c["commit_sha"].as_str().is_some_and(|x| x.starts_with(p))
                        })
                    })
                    .cloned()
                    .ok_or_else(|| format!("unknown commit {sha:?}"))
            }
            "compare_commits" => {
                let (r, base, head) = (arg(a, "repo")?, arg(a, "base_sha")?, arg(a, "head_sha")?);
                repo_known(r)?;
                let commits: Vec<_> = rows("commits").filter(|c| c["repo"] == r).collect();
                let pos = |sha: &str| commits.iter().position(|c| c["commit_sha"] == sha);
                match (pos(base), pos(head)) {
                    (Some(b), Some(h)) if h <= b => {
                        Ok(json!({"repo": r, "commits": commits[h..b].to_vec()}))
                    }
                    _ => Err(format!("cannot compare {base}..{head} in {r}")),
                }
            }
            "list_pull_requests" => {
                let r = arg(a, "repo")?;
                repo_known(r)?;
                let state = a.get("state").and_then(Value::as_str).unwrap_or("all");
                Ok(
                    json!({"pull_requests": rows("pull_requests").filter(|p| p["repo"] == r && (state == "all" || p["state"] == state)).collect::<Vec<_>>()}),
                )
            }
            "get_file" => {
                let (r, path) = (arg(a, "repo")?, arg(a, "path")?);
                repo_known(r)?;
                Err(format!("{path} not found in {r}"))
            }
            _ => Err(format!("unknown tool {name}")),
        }
    };
    Fixture {
        tools,
        handler: Arc::new(handler),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(f: &Fixture, name: &str, args: Value) -> Result<Value, String> {
        (f.handler)(name, args.as_object().unwrap())
    }

    #[test]
    fn incident_story_is_consistent_across_servers() {
        let (d, o, v) = (deploy(), observability(), vcs());
        let svc = call(&d, "search", json!({"query": "checkout"})).unwrap();
        assert_eq!(svc["services"][0]["service_id"], "svc-checkout");
        let deps = call(
            &d,
            "list_deployments",
            json!({"service_id": "svc-checkout"}),
        )
        .unwrap();
        let latest = &deps["deployments"][0];
        assert_eq!(
            (latest["deployment_id"].as_str(), latest["status"].as_str()),
            (Some("dep-4821"), Some("failed"))
        );
        assert!(
            latest.get("failure_reason").is_none(),
            "list hides details; get_deployment has them"
        );
        let logs = call(
            &o,
            "get_logs",
            json!({"service_id": "svc-checkout", "deployment_id": "dep-4821", "level": "error"}),
        )
        .unwrap();
        assert_eq!(logs["lines"].as_array().unwrap().len(), 2);
        assert!(logs.to_string().contains("STRIPE_WEBHOOK_SECRET"));
        let commit = call(
            &v,
            "get_commit",
            json!({"commit_sha": latest["commit_sha"]}),
        )
        .unwrap();
        assert_eq!(commit["pull_request"], 1287);
        let cmp = call(
            &v,
            "compare_commits",
            json!({"repo": "acme/checkout-api", "base_sha": "41d7e0c", "head_sha": "9f3c2ab"}),
        )
        .unwrap();
        assert_eq!(cmp["commits"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn non_ascii_commit_sha_is_an_error_not_a_panic() {
        assert!(call(&vcs(), "get_commit", json!({"commit_sha": "abcdef€"})).is_err());
    }

    #[test]
    fn rollback_task_has_a_real_target() {
        // search-api serves its newest deployment, so rolling back to the previous one changes something.
        let d = deploy();
        let st = call(&d, "get_status", json!({"service_id": "svc-search"})).unwrap();
        assert_eq!(st["serving"]["deployment_id"], "dep-4818");
        let deps = call(&d, "list_deployments", json!({"service_id": "svc-search"})).unwrap();
        assert_eq!(deps["deployments"][1]["deployment_id"], "dep-4702");
    }

    #[test]
    fn errors_guide_the_caller() {
        let e = call(
            &deploy(),
            "list_deployments",
            json!({"service_id": "checkout-api"}),
        )
        .unwrap_err();
        assert!(e.contains("deploy/search"), "{e}");
        assert!(call(&vcs(), "get_status", json!({"repo": "nope"})).is_err());
        assert!(
            call(
                &observability(),
                "query_metric",
                json!({"service_id": "svc-search", "metric": "bogus"})
            )
            .is_err()
        );
    }
}
