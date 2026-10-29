//! Offline retrieval benchmark: for each catalog size and discovery mode, does the returned
//! tool set (at most `LIMIT` tools) contain every tool the task needs? No LLM involved.

use std::path::Path;

use serde::Serialize;
use serde_json::json;

use super::{Task, lens_for_size, load_tasks, percentile, unix_now};
use crate::{
    config::Config,
    lens::{DiscoverOpts, Mode},
};

const LIMIT: usize = 8;
const REPS: usize = 5;

#[derive(Serialize)]
struct TaskResult {
    task: String,
    recall: f64,
    missing: Vec<String>,
    returned: Vec<String>,
}

#[derive(Serialize)]
struct ModeResult {
    mode: Mode,
    deps: bool,
    recall_mean: f64,
    /// Fraction of tasks where every gold tool was returned.
    complete_rate: f64,
    latency_p50_ms: f64,
    latency_p95_ms: f64,
    tasks: Vec<TaskResult>,
}

pub async fn run(
    cfg: &Config,
    tasks: &Path,
    sizes: &[usize],
    out: &Path,
    db: Option<&Path>,
) -> anyhow::Result<()> {
    let tasks = load_tasks(tasks)?;
    std::fs::create_dir_all(out)?;
    // Persistent store: caches embeddings (and a lazy catalog's tool index) across runs.
    // Nothing is invoked here, so no learned edges can accumulate.
    let db = db.map_or_else(|| out.join(".retrieval-cache.db"), Path::to_path_buf);
    // Size 0 means "the configured servers only", so no baseline count is needed.
    let base = if sizes.iter().any(|s| *s > 0) {
        lens_for_size(cfg, 0, 0, &db).await?.tools().len()
    } else {
        0
    };
    let mut results = vec![];
    for &size in sizes {
        let lens = lens_for_size(cfg, base, size, &db).await?;
        let semantic = lens
            .prepare_semantic()
            .await
            .map(|_| ())
            .map_err(|e| format!("{e:#}"));
        if let Err(e) = &semantic {
            eprintln!(
                "WARNING semantic retrieval unavailable, skipping semantic/hybrid modes: {e}"
            );
        }
        let mut modes = vec![];
        for mode in [Mode::Keyword, Mode::Semantic, Mode::Hybrid] {
            if mode != Mode::Keyword && semantic.is_err() {
                continue;
            }
            for deps in [false, true] {
                modes.push(
                    eval_mode(
                        &lens,
                        &tasks,
                        DiscoverOpts {
                            limit: LIMIT,
                            mode,
                            deps,
                        },
                    )
                    .await?,
                );
            }
        }
        println!("\n{size} tools ({} actual)", lens.tools().len());
        println!(
            "  {:<10} {:<5} {:>7} {:>9} {:>9} {:>9}",
            "mode", "deps", "recall", "complete", "p50 ms", "p95 ms"
        );
        for m in &modes {
            println!(
                "  {:<10} {:<5} {:>7.2} {:>9.2} {:>9.3} {:>9.3}",
                format!("{:?}", m.mode).to_lowercase(),
                m.deps,
                m.recall_mean,
                m.complete_rate,
                m.latency_p50_ms,
                m.latency_p95_ms
            );
        }
        results.push(json!({"size": size, "tools": lens.tools().len(), "semantic_error": semantic.err(), "modes": modes}));
    }
    let report =
        json!({"generated_unix": unix_now(), "limit": LIMIT, "reps": REPS, "sizes": results});
    let path = out.join("retrieval.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report)?)?;
    println!("\nwrote {}", path.display());
    Ok(())
}

async fn eval_mode(
    lens: &crate::lens::Lens,
    tasks: &[Task],
    opts: DiscoverOpts,
) -> anyhow::Result<ModeResult> {
    let mut latencies = vec![];
    let mut per_task = vec![];
    for t in tasks {
        let mut returned = vec![];
        for _ in 0..REPS {
            let d = lens.discover("bench-retrieval", &t.prompt, opts).await?;
            latencies.push(d.latency_ms);
            returned = d.tools.into_iter().map(|h| h.id).collect();
        }
        let missing: Vec<String> = t
            .gold
            .iter()
            .filter(|g| !returned.contains(g))
            .cloned()
            .collect();
        let recall = 1.0 - missing.len() as f64 / t.gold.len() as f64;
        per_task.push(TaskResult {
            task: t.id.clone(),
            recall,
            missing,
            returned,
        });
    }
    latencies.sort_by(f64::total_cmp);
    let n = per_task.len() as f64;
    Ok(ModeResult {
        mode: opts.mode,
        deps: opts.deps,
        recall_mean: per_task.iter().map(|t| t.recall).sum::<f64>() / n,
        complete_rate: per_task.iter().filter(|t| t.missing.is_empty()).count() as f64 / n,
        latency_p50_ms: percentile(&latencies, 0.5),
        latency_p95_ms: percentile(&latencies, 0.95),
        tasks: per_task,
    })
}
