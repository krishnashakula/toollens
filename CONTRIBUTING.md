# Contributing

Thanks for helping. Issues, bug reports and pull requests are all welcome.

## Build and test

These are exactly what CI runs (on Ubuntu and Windows, plus `cargo test` on the minimum Rust, 1.89):

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

The tests need nothing external: the end-to-end tests start the gateway and its simulated servers
from the crate's own binary. Semantic retrieval needs [Ollama](https://ollama.com) with
`nomic-embed-text`, but no test depends on it.

## Ground rules

- **Discovery never grants permission.** Policy is checked in `Lens::invoke` and nowhere else.
- **Upstream servers never inherit the gateway's environment**; only `INHERITED_ENV` plus each
  server's configured `env` (`src/upstream.rs`).
- **The audit log stores argument names, never values.**
- **In `serve` mode stdout is the MCP channel**: log to stderr only.
- **Bug fixes start with a failing reproduction**, and every real bug gets an entry in
  [BUGS.md](BUGS.md): symptom, cause, fix with file and line, and the command that verifies it.
- `bench agent` calls the Claude API and costs money. Don't tune discovery against
  `bench/tasks.json` or `bench/real/tasks.json`: add held-out tasks first.

[CLAUDE.md](CLAUDE.md) has the full command reference, including the real-world benchmark.
