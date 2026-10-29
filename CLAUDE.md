# ToolLens

Rust MCP gateway (rmcp 3.7, Tokio, rusqlite, tracing). It connects to many upstream MCP servers and
exposes exactly three tools: `discover_tools`, `inspect_tool` and `invoke_tool`. Discovery is
keyword/semantic retrieval plus a tool dependency graph; execution is policy-checked and
schema-validated. One binary is the gateway, the simulated upstream servers, the benchmarks and the
demo console.

## Commands (run from `toollens/`)

```powershell
cargo build --release
cargo test                                   # unit tests + tests/gateway_stdio.rs (real MCP over stdio)
cargo fmt --check; cargo clippy --all-targets -- -D warnings
./target/release/toollens serve --config examples/gateway.json            # MCP gateway on stdio
./target/release/toollens discover --config examples/gateway.json --goal "investigate a failed deployment" --noise 982
./target/release/toollens bench retrieval --config examples/gateway.json --tasks bench/tasks.json   # free, needs Ollama for semantic modes
./target/release/toollens bench agent --config examples/gateway.json --tasks bench/tasks.json       # COSTS MONEY (ANTHROPIC_API_KEY)
./target/release/toollens console --config examples/gateway.json --noise 982                        # prints http://127.0.0.1:7300/#token=…; /api needs that token
cd demo; npm ci; npx playwright install chromium; node record.mjs   # 4K60 video; needs the console on bench/real/catalog.json, $env:TOOLLENS_URL (the printed URL) and $env:FFMPEG (STORYBOARD.md)
```

Semantic retrieval needs Ollama with `nomic-embed-text` (`ollama pull nomic-embed-text`);
override with `OLLAMA_HOST` and `TOOLLENS_EMBED_MODEL`.

### Real-world catalog (110 vendor MCP servers, Docker sandbox, dummy credentials)

```powershell
docker build -t toollens-sandbox:1 -f bench/real/sandbox.Dockerfile bench/real
python bench/real/select_servers.py verify                       # servers.json (npm + PyPI lookups)
python bench/real/select_servers.py config                       # gateway.json
./target/release/toollens index --config bench/real/gateway.json > bench/real/index-report.json
python bench/real/select_servers.py config bench/real/index-report.json   # learn fixes, re-run index until stable
python bench/real/select_servers.py finalize bench/real/index-report.json # catalog.json = indexed servers only
./target/release/toollens bench retrieval --config bench/real/catalog.json --db bench/real/.toollens.db --tasks bench/real/tasks.json --sizes 0 --out bench/results/real
./target/release/toollens bench agent --config bench/real/catalog.json --db bench/real/.toollens.db --tasks bench/real/tasks.json --sizes 0 --arms lens,native,full --out bench/results/real   # COSTS MONEY
docker rm -f $(docker ps -q --filter label=toollens-real=1)      # containers a timed-out start left behind
```

`catalog.json` is lazy: tool lists come from the index cache in `.toollens.db`, servers start on
first call (at most `maxLive` at once). Use `--dry-run` on `bench agent` to measure per-arm tool
tokens for free.

## Where things are (`src/`)

- `lens.rs`: the engine. The MCP server, console and benchmark all go through it.
- `gateway.rs`: the MCP face.
- `meta.rs`: the contract of the three tools, shared by the gateway and the agent benchmark.
- `upstream.rs`: stdio clients with timeouts, cancellation and a bounded number of calls in flight.
- `catalog.rs`: `ToolRecord`.
- `store.rs`: SQLite (provenance, embeddings cache, audit log, observed edges).
- `keyword.rs` (BM25) and `semantic.rs` (Ollama): retrieval.
- `graph.rs` (schema edges, observed edges, session memory) and `discover.rs` (selection): the dependency graph and how tools are picked from it.
- `policy.rs`, `validate.rs`: permission checks and schema validation.
- `fixtures/`: simulated servers.
- `bench/`: the benchmarks.
- `console/`: axum and a single `index.html`.

## Rules

- Discovery never grants permission. Policy is checked in `Lens::invoke` and nowhere else; keep
  discovery about relevance only. Server annotations are untrusted: they may only tighten policy.
- The audit log stores argument *names*, never values (they can carry secrets).
- In `serve` and `fixture` modes stdout is the MCP channel: log to stderr only.
- `bench agent` spends real money (full 4-size sweep ≈ $7.60 on claude-opus-5-5). Ask first; use
  `--only <task-id>` to re-run a subset (the summary re-aggregates every transcript on disk).
- `bench/tasks.json` and `bench/real/tasks.json` were written alongside the system. Do not tune
  discovery against them; add held-out tasks first.
- Upstream servers never inherit the gateway's environment (API keys); only `INHERITED_ENV` plus
  each server's configured `env` (src/upstream.rs). Keep it that way.
- Windows: the release exe is locked while `console` or `bench` runs; stop them before rebuilding.
- After any console or results change, re-record the video per `demo/STORYBOARD.md`.
- Log every real bug in `BUGS.md` with its reproduction.
