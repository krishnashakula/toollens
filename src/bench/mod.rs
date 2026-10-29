//! Benchmarks: `retrieval` (offline, no LLM) and `agent` (Claude, full catalog vs ToolLens).

pub mod agent;
pub mod claude;
pub mod retrieval;

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::{config::Config, lens::Lens};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    /// Tools a correct solution needs; retrieval recall is measured against these.
    pub gold: Vec<String>,
    /// Other tools that are reasonable to call; anything outside gold + acceptable is an incorrect selection.
    #[serde(default)]
    pub acceptable: Vec<String>,
    #[serde(default)]
    pub expect_all: Vec<String>,
    #[serde(default)]
    pub expect_any: Vec<String>,
    #[serde(default)]
    pub expect_none: Vec<String>,
    #[serde(default)]
    pub grade: Grade,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Grade {
    /// The final answer must contain the expected facts.
    #[default]
    Answer,
    /// The agent must call a gold tool with schema-valid arguments. Used when the service needs
    /// a real account: with dummy credentials the call itself fails authentication.
    Select,
}

impl Task {
    /// A gold tool reached the upstream server: the gateway did not reject it as unknown,
    /// invalid or denied (an upstream auth failure still proves the selection was right).
    pub fn selected(&self, attempts: &[(&str, &str)]) -> bool {
        attempts.iter().any(|(tool, outcome)| {
            self.gold.iter().any(|g| g == tool)
                && matches!(*outcome, "ok" | "tool_error" | "failed")
        })
    }

    /// Deterministic grading of the final answer (case-insensitive substrings).
    pub fn grade(&self, answer: &str) -> bool {
        let a = answer.to_lowercase();
        let has = |s: &String| a.contains(&s.to_lowercase());
        self.expect_all.iter().all(has)
            && (self.expect_any.is_empty() || self.expect_any.iter().any(has))
            && !self.expect_none.iter().any(has)
    }

    pub fn is_reasonable(&self, tool_id: &str) -> bool {
        self.gold
            .iter()
            .chain(&self.acceptable)
            .any(|t| t == tool_id)
    }
}

pub fn load_tasks(path: &Path) -> anyhow::Result<Vec<Task>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// A gateway over the configured servers plus enough distractors to reach `size` tools.
/// Learning is off: every run starts from the same cold graph, so one task's successful
/// sequence cannot leak into the next.
pub async fn lens_for_size(
    cfg: &Config,
    base_tools: usize,
    size: usize,
    db: &Path,
) -> anyhow::Result<Lens> {
    let noise = size.saturating_sub(base_tools);
    let lens = Lens::start(&cfg.clone().with_noise(noise), db)
        .await?
        .learning(false);
    anyhow::ensure!(
        lens.unavailable.is_empty(),
        "servers unavailable: {:?}",
        lens.unavailable
    );
    Ok(lens)
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_grading_counts_only_forwarded_gold_calls() {
        let t: Task = serde_json::from_value(serde_json::json!({
            "id": "t", "prompt": "p", "gold": ["github/create_issue"], "grade": "select"
        }))
        .unwrap();
        assert_eq!(t.grade, Grade::Select);
        assert!(
            t.selected(&[("github/create_issue", "tool_error")]),
            "auth failure still proves selection"
        );
        assert!(
            !t.selected(&[("github/create_issue", "invalid")]),
            "schema-invalid arguments do not count"
        );
        assert!(!t.selected(&[("gitlab/create_issue", "ok")]));
        assert!(!t.selected(&[]));
    }

    #[test]
    fn grading_rules() {
        let t: Task = serde_json::from_value(serde_json::json!({
            "id": "t", "prompt": "p", "gold": ["a/b"],
            "expect_all": ["9F3C2AB"], "expect_any": ["x", "y"], "expect_none": ["not passing"]
        }))
        .unwrap();
        assert!(t.grade("commit 9f3c2ab, y"));
        assert!(!t.grade("commit 9f3c2ab"));
        assert!(!t.grade("9f3c2ab x but CI is not passing"));
        assert!(t.is_reasonable("a/b") && !t.is_reasonable("a/c"));
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.5), 3.0);
        assert_eq!(percentile(&[], 0.9), 0.0);
    }

    #[test]
    fn shipped_tasks_parse_and_reference_real_fixture_tools() {
        let tasks = load_tasks(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/bench/tasks.json"
        )))
        .unwrap();
        assert_eq!(tasks.len(), 8);
        let ids: Vec<String> = [
            crate::fixtures::infra::deploy(),
            crate::fixtures::infra::observability(),
            crate::fixtures::infra::vcs(),
        ]
        .iter()
        .zip(["deploy", "observability", "vcs"])
        .flat_map(|(f, s)| f.tool_names().into_iter().map(move |n| format!("{s}/{n}")))
        .collect();
        for t in &tasks {
            for id in t.gold.iter().chain(&t.acceptable) {
                assert!(
                    ids.contains(id),
                    "task {} references unknown tool {id}",
                    t.id
                );
            }
        }
    }
}
