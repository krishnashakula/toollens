//! Execution policy. Checked on every invoke and nowhere else: discovery never grants permission.

use serde::{Deserialize, Serialize};

use crate::catalog::ToolRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Default_ {
    Allow,
    Deny,
}

// A misspelled key ("denylist") must fail to load, not silently drop a rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Policy {
    /// Decision for tools no rule matches. Fails closed when the config omits it.
    pub default: Default_,
    /// Globs over tool ids (`server/tool`); `*` matches any run of characters.
    pub allow: Vec<String>,
    /// Deny globs win over everything, including `allow`.
    pub deny: Vec<String>,
    /// Deny tools whose server marks them destructive unless an `allow` glob names them.
    /// Annotations are untrusted hints, so they may only ever tighten the policy.
    pub deny_destructive: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            default: Default_::Deny,
            allow: vec![],
            deny: vec![],
            deny_destructive: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "lowercase")]
pub enum Verdict {
    Allow { rule: String },
    Deny { rule: String },
}

impl Verdict {
    pub fn allowed(&self) -> bool {
        matches!(self, Verdict::Allow { .. })
    }
}

impl Policy {
    pub fn check(&self, tool: &ToolRecord) -> Verdict {
        if let Some(g) = self.deny.iter().find(|g| glob(g, &tool.id)) {
            return Verdict::Deny {
                rule: format!("deny \"{g}\""),
            };
        }
        let allowed_by = self.allow.iter().find(|g| glob(g, &tool.id));
        if self.deny_destructive && tool.destructive_hint() && allowed_by.is_none() {
            return Verdict::Deny {
                rule: "denyDestructive (server marks this tool destructive)".into(),
            };
        }
        if let Some(g) = allowed_by {
            return Verdict::Allow {
                rule: format!("allow \"{g}\""),
            };
        }
        match self.default {
            Default_::Allow => Verdict::Allow {
                rule: "default allow".into(),
            },
            Default_::Deny => Verdict::Deny {
                rule: "default deny".into(),
            },
        }
    }
}

/// `*`-only glob, matched against the whole string.
pub fn glob(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == s;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !s.starts_with(first) || s.len() < first.len() + last.len() || !s.ends_with(last) {
        return false;
    }
    let mut rest = &s[first.len()..s.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn misspelled_policy_keys_fail_to_load() {
        let typo =
            serde_json::from_value::<Policy>(json!({"default": "allow", "denylist": ["a/*"]}));
        assert!(typo.unwrap_err().to_string().contains("denylist"));
        assert!(serde_json::from_value::<Policy>(json!({"denyDestructive": false})).is_ok());
    }

    fn tool(id: &str, destructive: bool) -> ToolRecord {
        let (server, name) = id.split_once('/').unwrap();
        ToolRecord {
            id: id.into(),
            server: server.into(),
            name: name.into(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
            annotations: destructive
                .then(|| json!({"destructiveHint": true, "readOnlyHint": false})),
            schema_hash: String::new(),
        }
    }

    #[test]
    fn glob_cases() {
        assert!(glob("deploy/*", "deploy/rollback"));
        assert!(glob("*/delete_*", "crm/delete_lead"));
        assert!(glob("a*b*c", "aXbYc"));
        assert!(!glob("a*b*c", "aXcYb"));
        assert!(!glob("deploy/*", "vcs/search"));
        assert!(glob("exact/name", "exact/name"));
        assert!(!glob("ab*ba", "aba")); // prefix and suffix may not overlap
        assert!(glob("*", ""));
    }

    #[test]
    fn deny_wins_then_destructive_then_allow_then_default() {
        let p = Policy {
            default: Default_::Allow,
            allow: vec!["deploy/*".into()],
            deny: vec!["deploy/rollback".into()],
            deny_destructive: true,
        };
        assert!(!p.check(&tool("deploy/rollback", false)).allowed());
        assert!(
            p.check(&tool("deploy/restart", true)).allowed(),
            "explicit allow overrides hint"
        );
        assert!(!p.check(&tool("crm/delete_lead", true)).allowed());
        assert!(p.check(&tool("crm/get_lead", false)).allowed());
    }

    #[test]
    fn missing_policy_fails_closed() {
        assert!(!Policy::default().check(&tool("a/b", false)).allowed());
    }
}
