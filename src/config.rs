//! Gateway config. `mcpServers` uses the same shape as Claude Desktop / Claude Code
//! so an existing client config can be pointed at ToolLens unchanged.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::policy::Policy;

/// Tools per synthetic noise server; `with_noise` spreads distractors over several servers.
pub const NOISE_PER_SERVER: usize = 100;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub mcp_servers: BTreeMap<String, ServerConfig>,
    #[serde(default)]
    pub policy: Policy,
    /// Start servers on first use instead of at startup; tool lists come from the index cache.
    #[serde(default)]
    pub lazy: bool,
    /// Upper bound on running servers in lazy mode; least recently used are stopped first.
    #[serde(default = "default_max_live")]
    pub max_live: usize,
    /// Ignore the tool cache and start every server again (set by `--reindex`).
    #[serde(default)]
    pub reindex: bool,
}

fn default_max_live() -> usize {
    12
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Executable to spawn. `@self` means this toollens binary (used for the fixtures).
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl ServerConfig {
    /// This server with `${VAR}` references expanded from the gateway's environment. Only the
    /// variables the config names are read; the server still never inherits the environment.
    pub fn resolved(&self) -> anyhow::Result<ServerConfig> {
        let var = |name: &str| std::env::var(name).ok();
        Ok(ServerConfig {
            command: expand(&self.command, var)?,
            args: self
                .args
                .iter()
                .map(|a| expand(a, var))
                .collect::<anyhow::Result<_>>()?,
            env: self
                .env
                .iter()
                .map(|(k, v)| Ok((k.clone(), expand(v, var)?)))
                .collect::<anyhow::Result<_>>()?,
        })
    }
}

/// Remote servers (`url`, no `command`) appear in client configs; ToolLens fronts stdio servers.
pub fn is_remote(server: &serde_json::Value) -> bool {
    server.get("command").is_none()
}

/// Expands `${VAR}` and `${VAR:-default}` the way Claude Code does in `.mcp.json`.
pub fn expand(s: &str, var: impl Fn(&str) -> Option<String>) -> anyhow::Result<String> {
    substitute(s, |inner| {
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (inner, None),
        };
        var(name)
            .or(default.map(str::to_string))
            .with_context(|| format!("environment variable {name} is not set"))
    })
}

/// The text inside every `${...}` in `s`, e.g. `TOKEN` or `URL:-http://localhost`.
pub fn env_refs(s: &str) -> Vec<String> {
    let mut refs = vec![];
    // An unclosed `${` is reported when the gateway expands the string.
    substitute(s, |inner| {
        refs.push(inner.to_string());
        Ok(String::new())
    })
    .ok();
    refs
}

/// Replaces each `${...}` with what `f` returns for the text inside it.
fn substitute(
    s: &str,
    mut f: impl FnMut(&str) -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let end = after
            .find('}')
            .with_context(|| format!("unclosed `${{` in {s:?}"))?;
        out.push_str(&f(&after[..end])?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config = serde_json::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Loads any file with `mcpServers`: a ToolLens config, Claude Code's `.mcp.json` or
    /// `~/.claude.json` (user-scope servers), Claude Desktop's config. Remote servers are left
    /// out and returned by name.
    pub fn load_client(path: &Path) -> anyhow::Result<(Self, Vec<String>)> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut v: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        let servers = v
            .get_mut("mcpServers")
            .and_then(serde_json::Value::as_object_mut)
            .with_context(|| format!("{} has no mcpServers object", path.display()))?;
        let remote: Vec<String> = servers
            .iter()
            .filter(|(_, s)| is_remote(s))
            .map(|(k, _)| k.clone())
            .collect();
        servers.retain(|_, s| !is_remote(s));
        if servers.is_empty() {
            bail!(
                "{} has no stdio servers (remote servers ToolLens cannot front: {remote:?})",
                path.display()
            );
        }
        let cfg: Config = serde_json::from_value(v)
            .with_context(|| format!("parsing config {}", path.display()))?;
        cfg.validate()?;
        Ok((cfg, remote))
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.mcp_servers.is_empty() {
            bail!("config has no mcpServers");
        }
        for name in self.mcp_servers.keys() {
            // Tool ids are `server/tool`, so a slash in the server name would make ids ambiguous.
            if name.is_empty() || name.contains('/') || name.chars().any(char::is_whitespace) {
                bail!("invalid server name {name:?}: must be non-empty, no '/' or whitespace");
            }
        }
        Ok(())
    }

    /// Adds `count` synthetic distractor tools, `NOISE_PER_SERVER` per noise server.
    pub fn with_noise(mut self, count: usize) -> Self {
        let mut left = count;
        let mut i = 0;
        while left > 0 {
            let n = left.min(NOISE_PER_SERVER);
            self.mcp_servers.insert(
                crate::fixtures::noise::server_name(i),
                ServerConfig {
                    command: "@self".into(),
                    args: vec![
                        "fixture".into(),
                        "noise".into(),
                        "--count".into(),
                        n.to_string(),
                        "--seed".into(),
                        i.to_string(),
                    ],
                    env: BTreeMap::new(),
                },
            );
            left -= n;
            i += 1;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Config {
        serde_json::from_str(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap()
    }

    #[test]
    fn noise_splits_into_servers_of_100() {
        let cfg = base().with_noise(250);
        assert_eq!(cfg.mcp_servers.len(), 4);
        assert_eq!(cfg.mcp_servers["revenue"].args[3], "50");
        assert_eq!(base().with_noise(0).mcp_servers.len(), 1);
    }

    #[test]
    fn expands_env_references_like_claude_code() {
        let var = |n: &str| (n == "TOKEN").then(|| "s3cret".to_string());
        assert_eq!(expand("Bearer ${TOKEN}", var).unwrap(), "Bearer s3cret");
        assert_eq!(
            expand("${NOPE:-sqlite://x}/db", var).unwrap(),
            "sqlite://x/db"
        );
        assert_eq!(expand("plain $TOKEN", var).unwrap(), "plain $TOKEN");
        let e = expand("${NOPE}", var).unwrap_err().to_string();
        assert!(e.contains("NOPE") && !e.contains("s3cret"), "{e}");
        assert!(expand("${TOKEN", var).is_err());
        assert_eq!(env_refs("${A} and ${B:-x}"), ["A", "B:-x"]);
    }

    #[test]
    fn client_configs_keep_stdio_servers_and_name_remote_ones() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".mcp.json");
        std::fs::write(&p, r#"{"mcpServers":{"a":{"type":"stdio","command":"x"},"web":{"type":"http","url":"https://e.x/mcp"}},"theme":"dark"}"#).unwrap();
        let (cfg, remote) = Config::load_client(&p).unwrap();
        assert_eq!(cfg.mcp_servers.keys().collect::<Vec<_>>(), ["a"]);
        assert_eq!(remote, ["web"]);
        std::fs::write(&p, r#"{"mcpServers":{"web":{"url":"https://e.x/mcp"}}}"#).unwrap();
        assert!(Config::load_client(&p).is_err(), "nothing to front");
    }

    #[test]
    fn rejects_slash_in_server_name() {
        let cfg: Config =
            serde_json::from_str(r#"{"mcpServers":{"a/b":{"command":"x"}}}"#).unwrap();
        assert!(cfg.validate().is_err());
    }
}
