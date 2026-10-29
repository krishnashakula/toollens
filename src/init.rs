//! `toollens init`: put the stdio servers of an existing client config (Claude Code, Claude
//! Desktop) behind ToolLens. The gateway config gets the server entries verbatim, so `${VAR}`
//! references stay references; the client keeps its remote servers and gets one `toollens`
//! entry in place of the rest. `--write` applies the client change, keeping a backup.

use std::{io::Write as _, path::Path};

use anyhow::{Context, bail, ensure};
use serde_json::{Map, Value, json};

use crate::config::{env_refs, is_remote};

pub struct Plan {
    pub gateway: Value,
    pub client: Value,
    pub moved: Vec<String>,
    pub remote: Vec<String>,
}

pub fn plan(client: &Value, exe: &str, gateway_path: &str) -> anyhow::Result<Plan> {
    let servers = client
        .get("mcpServers")
        .and_then(Value::as_object)
        .context("no mcpServers object")?;
    if servers.contains_key("toollens") {
        bail!("this config already has a `toollens` server");
    }
    let (remote, stdio): (Map<String, Value>, Map<String, Value>) = servers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .partition(|(_, s)| is_remote(s));
    ensure!(
        !stdio.is_empty(),
        "no stdio servers to move (remote servers stay in the client)"
    );
    // The client expands `${VAR}` for its own servers; pass each one through to ToolLens, which
    // expands it again for the server it belongs to.
    let mut env = Map::new();
    for s in stdio.values() {
        for inner in strings(s).iter().flat_map(|s| env_refs(s)) {
            let name = inner.split_once(":-").map_or(inner.as_str(), |(n, _)| n);
            env.entry(name.to_string())
                .or_insert_with(|| json!(format!("${{{inner}}}")));
        }
    }
    let mut entry = json!({"command": exe, "args": ["serve", "--config", gateway_path]});
    if !env.is_empty() {
        entry["env"] = Value::Object(env);
    }
    let moved: Vec<String> = stdio.keys().cloned().collect();
    let names: Vec<String> = remote.keys().cloned().collect();
    let mut new_servers = remote;
    new_servers.insert("toollens".into(), entry);
    let mut client = client.clone();
    client["mcpServers"] = Value::Object(new_servers);
    Ok(Plan {
        // A starting policy: what the client could already call, minus tools their own servers
        // mark destructive. Review it.
        gateway: json!({"mcpServers": stdio, "policy": {"default": "allow", "denyDestructive": true}}),
        client,
        moved,
        remote: names,
    })
}

/// Every string anywhere in a server entry (command, args, env values).
fn strings(v: &Value) -> Vec<&String> {
    match v {
        Value::String(s) => vec![s],
        Value::Array(a) => a.iter().flat_map(strings).collect(),
        Value::Object(o) => o.values().flat_map(strings).collect(),
        _ => vec![],
    }
}

pub fn run(from: &Path, out: Option<&Path>, write: bool) -> anyhow::Result<()> {
    // Claude Code rewrites ~/.claude.json while it runs, so an edit here could be lost or could
    // clobber its state; show the change instead.
    let claude_state = from.file_name().is_some_and(|n| n == ".claude.json");
    if write && claude_state {
        bail!(
            "--write does not edit ~/.claude.json (Claude Code rewrites it while running); run without --write and apply the change with Claude Code closed"
        );
    }
    let client: Value = serde_json::from_str(
        &std::fs::read_to_string(from).with_context(|| format!("reading {}", from.display()))?,
    )
    .with_context(|| format!("parsing {}", from.display()))?;
    let gateway_path = std::path::absolute(out.unwrap_or(&from.with_file_name("toollens.json")))?;
    let exe = std::path::absolute(std::env::current_exe()?)?;
    let plan = plan(
        &client,
        &exe.display().to_string(),
        &gateway_path.display().to_string(),
    )?;
    let backup = from.with_extension("json.bak");
    if write && backup.exists() {
        bail!("{} exists; move it away first", backup.display());
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&gateway_path)
        .with_context(|| {
            format!(
                "creating {} (pass --out to choose another file)",
                gateway_path.display()
            )
        })?
        .write_all(pretty(&plan.gateway)?.as_bytes())?;
    println!(
        "Wrote {}: {} servers behind ToolLens ({}). Policy: allow, except tools a server marks destructive.",
        gateway_path.display(),
        plan.moved.len(),
        plan.moved.join(", ")
    );
    if !plan.remote.is_empty() {
        println!(
            "Left in the client (remote servers): {}",
            plan.remote.join(", ")
        );
    }
    if write {
        std::fs::copy(from, &backup)?;
        // Write beside the file, then rename over it: a crash never leaves it half written.
        let tmp = from.with_extension("json.toollens-tmp");
        std::fs::write(&tmp, pretty(&plan.client)?)?;
        std::fs::rename(&tmp, from)?;
        println!(
            "Updated {} (backup: {}). Restart your client.",
            from.display(),
            backup.display()
        );
    } else {
        println!(
            "\nReplace \"mcpServers\" in {} with:\n\n{}\nor run again with --write to apply it (a backup is kept).",
            from.display(),
            serde_json::to_string_pretty(&plan.client["mcpServers"])?
        );
    }
    println!(
        "\nYour client now asks permission per invoke_tool call; the policy in {} decides which tools may run.\n\
         That file holds your server entries, including any keys in their env: keep it out of git.\n\
         See what it saves: toollens tax --config \"{}\"",
        gateway_path.display(),
        gateway_path.display()
    );
    Ok(())
}

fn pretty(v: &Value) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(v)? + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn moves_stdio_servers_keeps_remote_ones_and_passes_env_references_through() {
        let client = json!({
            "mcpServers": {
                "github": {"command": "npx", "args": ["-y", "gh"], "env": {"TOKEN": "${GH_TOKEN}"}},
                "db": {"type": "stdio", "command": "uvx", "args": ["db", "--url", "${DB_URL:-sqlite://x}"]},
                "web": {"type": "http", "url": "https://example.com/mcp"}
            },
            "theme": "dark"
        });
        let p = plan(&client, "/bin/toollens", "/cfg/toollens.json").unwrap();
        assert_eq!(p.moved, ["db", "github"]);
        assert_eq!(p.remote, ["web"]);
        assert_eq!(
            p.gateway["mcpServers"]["github"], client["mcpServers"]["github"],
            "copied verbatim"
        );
        assert_eq!(p.client["theme"], "dark");
        assert_eq!(p.client["mcpServers"]["web"], client["mcpServers"]["web"]);
        assert!(p.client["mcpServers"].get("github").is_none());
        assert_eq!(
            p.client["mcpServers"]["toollens"],
            json!({
                "command": "/bin/toollens",
                "args": ["serve", "--config", "/cfg/toollens.json"],
                "env": {"GH_TOKEN": "${GH_TOKEN}", "DB_URL": "${DB_URL:-sqlite://x}"}
            })
        );
        let gw: Config = serde_json::from_value(p.gateway.clone()).unwrap();
        assert!(gw.policy.deny_destructive);
        assert!(
            plan(&p.client, "x", "y").is_err(),
            "already behind ToolLens"
        );
        assert!(plan(&json!({"mcpServers": {"web": {"url": "u"}}}), "x", "y").is_err());
    }
}
