//! `toollens tax`: how many tokens of tool definitions a set of MCP servers adds to every
//! request, and what the same servers cost behind ToolLens. Estimated locally by default;
//! `--exact` counts with the Messages API's free count_tokens, which means sending the tool
//! definitions (names, descriptions, schemas; never env or args) to Anthropic.

use std::{collections::BTreeMap, fmt::Write, path::Path};

use crate::{
    bench::{agent, claude::Claude},
    config::Config,
    lens::Lens,
};

/// tokens ≈ OVERHEAD + definition characters / CHARS_PER_TOKEN, fitted to count_tokens on the
/// 110-server catalog (3,558,435 chars, 1,428,973 tokens) and ToolLens's three tools (1,807
/// chars, 951 tokens); on examples/gateway.json it is 2% low (2,741 vs 2,796).
const CHARS_PER_TOKEN: f64 = 2.49;
const OVERHEAD: f64 = 225.0;

pub struct Options<'a> {
    pub top: usize,
    pub exact: bool,
    pub model: &'a str,
}

pub async fn run(path: &Path, db: &Path, opts: Options<'_>) -> anyhow::Result<()> {
    let (cfg, remote) = Config::load_client(path)?;
    let lens = Lens::start(&cfg, db).await?;
    let mut per_tool = vec![];
    let mut rejected = vec![];
    for t in lens.tools() {
        let bad = agent::rejected_property_names(&t.input_schema);
        if bad.is_empty() {
            per_tool.push((t, chars(&agent::api_tool(t))));
        } else {
            rejected.push((t, bad));
        }
    }
    let catalog_chars: usize = per_tool.iter().map(|(_, c)| c).sum();
    let lens_chars: usize = agent::meta_tools(&lens).iter().map(chars).sum();
    let (total, lens_total) = if opts.exact {
        let claude = Claude::from_env()?;
        let full = agent::build_arm(&claude, &lens, "full", opts.model).await?;
        let meta = agent::build_arm(&claude, &lens, "lens", opts.model).await?;
        tracing::info!(
            catalog_chars,
            catalog_tokens = full.schema_tokens(),
            lens_chars,
            lens_tokens = meta.schema_tokens(),
            "count_tokens"
        );
        (full.schema_tokens() as f64, meta.schema_tokens() as f64)
    } else {
        (estimate(catalog_chars), estimate(lens_chars))
    };
    // Per server and per tool: the total split by share of definition size.
    let share = |c: usize| total * c as f64 / catalog_chars.max(1) as f64;
    let mut servers: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for t in lens.tools() {
        servers.entry(t.server.as_str()).or_default().0 += 1;
    }
    for (t, c) in &per_tool {
        servers.entry(t.server.as_str()).or_default().1 += c;
    }
    let mut servers: Vec<_> = servers.into_iter().collect();
    servers.sort_by_key(|(name, (_, c))| (std::cmp::Reverse(*c), *name));
    per_tool.sort_by_key(|(t, c)| (std::cmp::Reverse(*c), t.id.as_str()));

    let approx = if opts.exact { "" } else { "≈ " };
    let mut o = String::new();
    writeln!(o, "# MCP tool tax: {}\n", path.display())?;
    writeln!(
        o,
        "{} servers, {} tools: **{approx}{} tokens** of tool definitions with every request ({}).",
        servers.len(),
        thousands(lens.tools().len() as f64),
        thousands(total),
        if opts.exact {
            "count_tokens"
        } else {
            "estimated locally; `--exact` counts with the API"
        }
    )?;
    writeln!(
        o,
        "Through ToolLens the agent sees 3 tools: **{approx}{} tokens**.\n",
        thousands(lens_total)
    )?;
    writeln!(
        o,
        "| Server | Tools | Tokens per request |\n|---|---:|---:|"
    )?;
    for (name, (n, c)) in &servers {
        writeln!(o, "| {name} | {n} | ≈ {} |", thousands(share(*c)))?;
    }
    if opts.top > 0 && !per_tool.is_empty() {
        writeln!(o, "\n## Heaviest tools\n\n| Tool | Tokens |\n|---|---:|")?;
        for (t, c) in per_tool.iter().take(opts.top) {
            writeln!(o, "| {} | ≈ {} |", t.id, thousands(share(*c)))?;
        }
    }
    if !(rejected.is_empty() && lens.unavailable.is_empty() && remote.is_empty()) {
        writeln!(o, "\n## Problems\n")?;
    }
    if !rejected.is_empty() {
        writeln!(
            o,
            "- {} tools use property names the Claude API refuses; one of them in a request makes the \
             API reject the whole request (not counted above):",
            rejected.len()
        )?;
        for (t, bad) in &rejected {
            writeln!(o, "  - `{}`: `{}`", t.id, bad.join("`, `"))?;
        }
    }
    if !lens.unavailable.is_empty() {
        writeln!(o, "- {} servers failed to start:", lens.unavailable.len())?;
        for (name, error) in &lens.unavailable {
            let first: String = error
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect();
            writeln!(o, "  - `{name}`: {first}")?;
        }
    }
    if !remote.is_empty() {
        writeln!(
            o,
            "- Remote servers not measured (ToolLens fronts stdio servers): `{}`",
            remote.join("`, `")
        )?;
    }
    if lens_total >= total {
        writeln!(
            o,
            "\nWith this few tools ToolLens adds tokens instead of saving them; in the benchmarks it \
             pays off somewhere between 30 and 100 tools."
        )?;
    } else {
        writeln!(
            o,
            "\nShare: My MCP setup sends {approx}{} tokens of tool definitions with every request. \
             Through ToolLens: {approx}{}. https://github.com/krishnashakula/toollens",
            thousands(total),
            thousands(lens_total)
        )?;
    }
    print!("{o}");
    Ok(())
}

fn chars(v: &serde_json::Value) -> usize {
    v.to_string().chars().count()
}

fn estimate(chars: usize) -> f64 {
    if chars == 0 {
        return 0.0;
    }
    OVERHEAD + chars as f64 / CHARS_PER_TOKEN
}

fn thousands(n: f64) -> String {
    let digits = (n.round() as u64).to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0.0), "0");
        assert_eq!(thousands(951.4), "951");
        assert_eq!(thousands(1428973.0), "1,428,973");
        assert_eq!(thousands(100000.0), "100,000");
    }
}
