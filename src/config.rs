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

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config = serde_json::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
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
    fn rejects_slash_in_server_name() {
        let cfg: Config =
            serde_json::from_str(r#"{"mcpServers":{"a/b":{"command":"x"}}}"#).unwrap();
        assert!(cfg.validate().is_err());
    }
}
