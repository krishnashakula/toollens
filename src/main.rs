use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, bail};
use rmcp::ServiceExt;
use toollens::{
    config::Config,
    gateway::Gateway,
    lens::{DiscoverOpts, Lens, Mode},
};

const USAGE: &str = "\
toollens - MCP gateway: discover, inspect and invoke tools from a large catalog

USAGE:
  toollens serve    --config <file> [--db <file>] [--mode keyword|semantic|hybrid] [--noise N]
  toollens index    --config <file> [--reindex]     list every server's tools, report failures
  toollens discover --config <file> --goal <text> [--mode M] [--limit N] [--no-deps] [--noise N]
  toollens bench retrieval --config <file> --tasks <file> [--sizes 30,100,300,1000] [--db <file>] [--out <dir>]
  toollens bench agent     --config <file> --tasks <file> [--sizes ...] [--arms full,lens,native] [--model ID] [--only id,..] [--runs N] [--db <file>] [--dry-run] [--out <dir>]
  toollens console  --config <file> [--results <dir>] [--noise N] [--port 7300] [--model ID]
  toollens tax      --config <client config> [--top N] [--exact] [--db <file>]   tool-definition tokens per request
  toollens init     --from <client config> [--out <file>] [--write]              put a client's servers behind ToolLens
  toollens fixture  deploy|observability|vcs|noise [--count N] [--seed N]

Logs go to stderr (stdout carries the MCP protocol); filter with RUST_LOG.";

struct Args {
    positional: Vec<String>,
    flags: HashMap<String, String>,
    switches: Vec<String>,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut a = Args {
            positional: vec![],
            flags: HashMap::new(),
            switches: vec![],
        };
        let mut it = std::env::args().skip(1).peekable();
        while let Some(arg) = it.next() {
            match arg.strip_prefix("--") {
                Some(name)
                    if matches!(
                        name,
                        "no-deps" | "help" | "reindex" | "dry-run" | "exact" | "write"
                    ) =>
                {
                    a.switches.push(name.into())
                }
                Some(name) => {
                    let v = it
                        .next()
                        .with_context(|| format!("--{name} needs a value"))?;
                    a.flags.insert(name.into(), v);
                }
                None => a.positional.push(arg),
            }
        }
        Ok(a)
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.flags.get(k).map(String::as_str)
    }

    fn num(&self, k: &str, default: usize) -> anyhow::Result<usize> {
        self.get(k).map_or(Ok(default), |v| {
            v.parse().with_context(|| format!("--{k} must be a number"))
        })
    }

    fn config(&self) -> anyhow::Result<(Config, PathBuf)> {
        let path = PathBuf::from(self.get("config").context("--config <file> is required")?);
        let mut cfg = Config::load(&path)?.with_noise(self.num("noise", 0)?);
        cfg.reindex = self.switches.iter().any(|s| s == "reindex");
        Ok((cfg, path))
    }

    fn opts(&self) -> anyhow::Result<DiscoverOpts> {
        Ok(DiscoverOpts {
            limit: self.num("limit", 8)?,
            mode: self.get("mode").unwrap_or("keyword").parse()?,
            deps: !self.switches.iter().any(|s| s == "no-deps"),
        })
    }
}

/// Default database sits next to the config, so it does not depend on the launching client's cwd.
fn db_path(args: &Args, config: &Path) -> PathBuf {
    args.get("db")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.with_file_name(".toollens.db"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "toollens=info,warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse()?;
    let cmd: Vec<&str> = args.positional.iter().map(String::as_str).collect();
    if args.switches.iter().any(|s| s == "help") || cmd.is_empty() {
        println!("{USAGE}");
        return Ok(());
    }
    match cmd.as_slice() {
        ["fixture", kind] => {
            toollens::fixtures::serve(kind, args.num("count", 100)?, args.num("seed", 0)?).await
        }
        ["serve"] => {
            let (cfg, path) = args.config()?;
            let opts = args.opts()?;
            let lens = Arc::new(Lens::start(&cfg, &db_path(&args, &path)).await?);
            if opts.mode != Mode::Keyword {
                lens.prepare_semantic()
                    .await
                    .context("embedding the catalog for semantic discovery")?;
            }
            tracing::info!(
                tools = lens.tools().len(),
                servers = lens.servers().len(),
                unavailable = lens.unavailable.len(),
                "gateway ready"
            );
            Gateway::new(lens, opts)
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
            Ok(())
        }
        ["index"] => {
            let (cfg, path) = args.config()?;
            let lens = Lens::start(&cfg, &db_path(&args, &path)).await?;
            let report = serde_json::json!({
                "servers": lens.servers(),
                "tools": lens.tools().len(),
                "failed": lens.unavailable.iter().map(|(s, e)| serde_json::json!({"server": s, "error": e})).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        ["discover"] => {
            let (cfg, path) = args.config()?;
            let goal = args.get("goal").context("--goal <text> is required")?;
            let lens = Lens::start(&cfg, &db_path(&args, &path)).await?;
            let d = lens.discover("cli", goal, args.opts()?).await?;
            println!("{}", serde_json::to_string_pretty(&d)?);
            Ok(())
        }
        ["bench", which] => {
            let (cfg, _) = args.config()?;
            let tasks = PathBuf::from(args.get("tasks").context("--tasks <file> is required")?);
            let sizes: Vec<usize> = args
                .get("sizes")
                .unwrap_or("30,100,300,1000")
                .split(',')
                .map(|s| {
                    s.trim()
                        .parse()
                        .context("--sizes must be comma-separated numbers")
                })
                .collect::<anyhow::Result<_>>()?;
            let out = PathBuf::from(args.get("out").unwrap_or("bench/results"));
            match *which {
                "retrieval" => {
                    let db = args.get("db").map(Path::new);
                    toollens::bench::retrieval::run(&cfg, &tasks, &sizes, &out, db).await
                }
                "agent" => {
                    let arms: Vec<String> = args
                        .get("arms")
                        .unwrap_or("full,lens")
                        .split(',')
                        .map(str::to_string)
                        .collect();
                    let model = args.get("model").unwrap_or("claude-opus-5-5");
                    let opts = toollens::bench::agent::Options {
                        sizes: &sizes,
                        arms: &arms,
                        model,
                        out: &out,
                        only: args.get("only"),
                        db: args.get("db").map(Path::new),
                        dry_run: args.switches.iter().any(|s| s == "dry-run"),
                        runs: match args.get("runs") {
                            Some(_) => Some(args.num("runs", 1)?.max(1)),
                            None => None,
                        },
                    };
                    toollens::bench::agent::run(&cfg, &tasks, opts).await
                }
                other => bail!("unknown bench {other:?} (retrieval | agent)"),
            }
        }
        ["console"] => {
            let (cfg, path) = args.config()?;
            let results = PathBuf::from(args.get("results").unwrap_or("bench/results"));
            let port: u16 = args
                .num("port", 7300)?
                .try_into()
                .context("--port out of range")?;
            let lens = Arc::new(Lens::start(&cfg, &db_path(&args, &path)).await?);
            let model = args.get("model").unwrap_or("claude-opus-5-5").to_string();
            toollens::console::serve(lens, results, port, model).await
        }
        ["tax"] => {
            let path = PathBuf::from(args.get("config").context("--config <file> is required")?);
            // In memory by default: measuring a client config must not leave a database beside it.
            let db = PathBuf::from(args.get("db").unwrap_or(":memory:"));
            let opts = toollens::tax::Options {
                top: args.num("top", 5)?,
                exact: args.switches.iter().any(|s| s == "exact"),
                model: args.get("model").unwrap_or("claude-opus-5-5"),
            };
            toollens::tax::run(&path, &db, opts).await
        }
        ["init"] => {
            let from = PathBuf::from(
                args.get("from")
                    .context("--from <client config> is required")?,
            );
            let write = args.switches.iter().any(|s| s == "write");
            toollens::init::run(&from, args.get("out").map(Path::new), write)
        }
        _ => bail!("unknown command {:?}\n\n{USAGE}", args.positional.join(" ")),
    }
}
