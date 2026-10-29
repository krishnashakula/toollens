# ToolLens bug log

Newest first. One entry per bug: symptom, cause, fix, and the command that proves it.
Entries 015–026 came from a pre-release review of the whole codebase.

## 026 — A fixture server panicked on a non-ASCII commit sha

**Cause:** `&sha[..7]` slices bytes; byte 7 inside a multi-byte character panics. Repro:
`vcs/get_commit {"commit_sha": "abcdef€"}` → handler panic, the gateway reported a 30 s timeout.
**Fix:** `sha.get(..7)` (src/fixtures/infra.rs `get_commit`).
**Verified:** `cargo test --lib non_ascii_commit_sha_is_an_error_not_a_panic`.

## 025 — Console live runs: no concurrency limit, invisible errors, out-of-order polling

**Cause:** Every POST /api/live spawned another paid run; a run still going could be evicted from
memory (404); the UI ignored a failed POST and polled every 250 ms with overlapping requests, so
responses arriving out of order could apply events twice, and a 404 meant polling forever.
**Fix:** One live run at a time, 409 otherwise (src/console/live.rs `running`, `Running` guard);
the UI polls one request at a time and shows errors (src/console/index.html `runLive`).
**Verified:** a second POST while one ran → `409 {"error":"a live run is already in progress"}`
(tested with the full-catalog arm, which the API rejects unbilled).

## 024 — A 2xx Claude response with an unreadable body counted as an empty answer

**Cause:** `resp.json().await.unwrap_or(Value::Null)` turned a body read failure (connection reset
mid-body) into `Ok(Null)`: an empty final answer, graded FAIL and blamed on the model.
**Fix:** A body read failure on 2xx is an error; `retry-after` is capped at 120 s
(src/bench/claude.rs `post`).
**Verified:** not reproduced (needs a mid-body reset); none of the 148 saved transcripts show it.

## 023 — Edges learned in the console could leak into benchmarks

**Cause:** `learning(false)` stopped writing observed edges but `Lens::new` still loaded those
already in the store, and the console (learning on) shares `bench/real/.toollens.db` with the bench.
**Fix:** `learning(false)` also drops loaded edges (src/lens.rs `learning`).
**Verified:** the store holds 0 observed edges, so published results are unaffected; the fix is
not covered by a test.

## 022 — Benchmark re-runs counted calls from earlier runs

**Cause:** The audit session id was `bench-{size}-{arm}-{task}`, the same on every run, and the
audit log persists in `--db`, so an `--only` re-run also counted the previous run's calls (and
select-graded tasks are graded from that log). Repro: 6 of 84 real transcripts list more calls in
`invoked` than their steps made, e.g. `lens/github-issue`: two `github/create_issue`, one in steps.
**Fix:** The session id carries a per-run nonce (src/bench/agent.rs `episode`).
**Verified:** each of the 6 runs' own steps include its gold tool, so every grade stands and the
README's wrong-tool and first-pick figures are unchanged; only `invocations` in those 6 files is
inflated, left as recorded. The fixed path was not re-run (costs money).

## 021 — Cancelling invoke_tool did not stop it

**Cause:** rmcp only flags a client's `notifications/cancelled` on `context.ct`; the handler ignored
it, so a call still waiting on a server start or a permit ran after the user cancelled.
**Fix:** `call_tool` races the dispatch against `context.ct.cancelled()`; dropping the dispatch
drops the upstream call (src/gateway.rs `call_tool`).
**Verified:** not tested end to end (needs a client that cancels mid-call); builds, and
`tests/gateway_stdio.rs` passes.

## 020 — A misspelled policy key was silently ignored

**Cause:** `Policy` accepted unknown fields, so `"denylist": [...]` loaded as no deny rules: the
security config failed open.
**Fix:** `deny_unknown_fields` (src/policy.rs `Policy`).
**Verified:** `cargo test --lib misspelled_policy_keys_fail_to_load`; the example and both real
configs still load.

## 019 — A crashed upstream server was never restarted

**Cause:** `Pool::take_live` handed back the pooled client without checking its transport, and each
failed call moved it to most-recently-used, so LRU never evicted it. Repro: kill a fixture server,
then every call returns "Transport closed" until the gateway restarts.
**Fix:** `take_live` drops an entry whose transport has closed; the caller starts a fresh server
(src/upstream.rs `take_live`).
**Verified:** `cargo test --lib a_server_that_exited_is_dropped_not_reused` fails without the fix
("a dead server must not be reused") and passes with it.

## 018 — The audit log stored argument values

**Cause:** The rejection message went into `calls.detail`, and schema validation errors quote the
offending value (upstream error text can echo anything). Repro: `{"limit": "4111111111111111"}` →
`args=[limit] invalid arguments ...: /limit: "4111111111111111" is not of type "integer"`.
**Fix:** The audit keeps only the gateway's own messages (denied, unknown tool); the agent still
gets the full error (src/lens.rs `invoke`).
**Verified:** `cargo test --lib audit_log_never_stores_argument_values` fails without the fix
(prints the row above) and passes with it.

## 017 — Credentials in a server's launch args reached the model

**Cause:** `inspect_tool` provenance and spawn errors carried the full command line. Real configs
put secrets in args (connection strings, `--api-key`, auth headers). Repro: args
`--password hunter2-SECRET-ARG` → `inspect_tool` returned them. Paths also exposed user names.
**Fix:** `command_line` is the program's file name only (src/upstream.rs `command_line`). The
committed transcripts' provenance paths were redacted to `toollens.exe`.
**Verified:** `cargo test --lib command_line_never_carries_args_or_directories`.

## 016 — Any local process or web page could spend the user's API credits through the console

**Cause:** The console had no authentication; binding to 127.0.0.1 is not enough. Repro: from a
sandbox container, `fetch('http://host.docker.internal:7300/api/catalog')` → 200; POST /api/warm
with `Host: rebind.attacker.example:7300` → 200 (DNS rebinding). POST /api/live spends money.
**Fix:** A per-start token, printed only to the terminal as `http://127.0.0.1:7300/#token=…` and
required on every `/api` route (src/console/mod.rs `require_token`). The page reads it from the URL
fragment, which never reaches the server (src/console/index.html `TOKEN`); `demo/record.mjs` takes
it from `TOOLLENS_URL`.
**Verified:** no token, wrong token, the rebinding POST and the container → 401; right token → 200;
the browser loads every scene and the token leaves the address bar.

## 015 — Cargo.toml claimed Rust 1.88; it needs 1.89

**Cause:** rusqlite 0.41 requires 1.89. Repro: `cargo test --locked` in `rust:1.88-bookworm` →
"rustc 1.88.0 is not supported by the following package".
**Fix:** `rust-version = "1.89"` (Cargo.toml) and a CI job on 1.89 keeps it true.
**Verified:** `cargo test --locked` in `rust:1.89-bookworm` → all tests pass.

## 014 — The last scroll in the demo stopped dead mid-motion

**Cause:** `scrollToScene('real-charts')` aimed at scrollY 7229, but the page can only scroll to 6864.
The eased tween hit the browser's clamp about 43% of the way through, at speed, and froze for the rest.
Repro: in the 4K60 render, ffmpeg `mpdecimate` over the 57 frames of that scroll kept 25 distinct
frames; the catalog and real-scene scrolls kept 56 and 57.
**Fix:** The scroll target is clamped to the maximum scroll position (src/console/index.html
`scrollToScene`).
**Verified:** re-render, same check: 47 of 57 frames distinct (the rest are the sub-pixel ease-in/out
ends of a 174 px scroll); frames 3873/3890/3905/3929 show the scroll moving through all 0.9 s.

## 013 — The demo render froze for good at 64.5 s of 70, every process idle

**Cause:** A deadlock in the recorder. `page.evaluate(() => window.toollens.scrollToScene(...))`
returned the scroll tween's promise, so Playwright waited for it. The tween settles on
`requestAnimationFrame`, which runs on the virtual clock, and the clock only advances in `frame()`,
which could not run while the script waited. `scene()` used a block body and was fine; only the
`real-charts` scroll had the expression form. The bug predates the GPU work: the earlier CPU render
was stopped at 50 s, before this line. Repro: two `node record.mjs --reuse` runs stopped after
"60s rendered" with node, ffmpeg, Chromium and the GPU at 0% (one for three days). The same timeline
with no capture or encoder printed `64.55s f3873 real-charts` and then a 15 s timeout on that
evaluate. Ruled out: NVENC, which encoded 4,800 synthetic 4K JPEGs through the same command
(exit 0); ffmpeg's 2.7 GB was frames queued for encoding.
**Fix:** The `real-charts` scroll uses a block body (demo/record.mjs, last scene), with a comment on
`scene()` saying why. Also restored a 30 s timeout on frame capture (demo/record.mjs `capture`),
which the raw CDP call had lost when it replaced `page.screenshot`.
**Verified:** `node record.mjs --reuse` → "rendered 4185 frames (69.8s) in 505s" and
`ok: ... 3840x2160 @ 60 fps, 69.75s, full render`; past the former hang point with no stall.

## 012 — The 4K demo video looked like slow motion

**Cause:** Two things. (1) Capture: Playwright's `recordVideo` streams a JPEG per frame from the
browser; at 3840×2160 that throttles the page itself to about 12 rendered frames per second, so
motion judders whatever the encoder's 30 fps says. (2) Pacing: long scripted holds and replays of
old transcripts at half a second per step. Repro: a page counting `requestAnimationFrame` calls
rendered 58 frames in 5 s while being recorded at 4K (67 headed with the GPU).
**Fix:** `demo/record.mjs` now runs the demo tasks live first (real model, gateway and server calls,
each event time-stamped), then renders on Playwright's virtual clock: 1/60 s per frame, CSS
animations stepped from the same clock, one 4K screenshot per frame piped to x264. The live runs play
back at their recorded pace; nothing else is slowed down.
**Verified:** a virtual-clock probe ran exactly one `requestAnimationFrame` per frame and moved a CSS
transition to 918 px against about 900 px expected; `node record.mjs --check` checks size, 60 fps
and a decoded frame.

## 011 — Benchmark arms shared server state (memory graph leaked from one arm into the next)

**Cause:** One gateway (and its pool of running servers) served every arm of a size. The memory
server kept the entity the ToolLens arm created, so the native arm found "ToolLens" already stored,
wrote nothing, and was graded FAIL on `memory-remember`. Repro: first real run,
`bench/results/real/agent/0/native/memory-remember.json` invoked only `memory/search_nodes`.
**Fix:** A fresh gateway, and so fresh servers, per arm (src/bench/agent.rs `run`, `lens_for_size`
inside the arm loop); lazy catalogs reload from the index cache in seconds.
**Verified:** `toollens bench agent ... --arms native --only memory-remember` → PASS
(`memory/create_entities` called). The ToolLens arm ran first in the original run, so its result
was not affected.

## 010 — On a real 110-server catalog, dependency expansion lowered recall (0.75 → 0.67)

**Cause:** Schema edges linked tools of *different vendors* whenever field names matched, and
dependencies could fill every slot. In the synthetic suite shared ids (`deployment_id`) were
deliberate; across real vendors most matches are coincidence (`region`, `database`, `path`), so
Mailgun/Postman/MongoDB tools were pulled in as "dependencies" and pushed real matches such as
`fetch/fetch` and `clickhouse/run_query` out of the top 8. Repro: `toollens bench retrieval
--config bench/real/catalog.json --db bench/real/.toollens.db --tasks bench/real/tasks.json --sizes 0`
→ keyword 0.75, keyword+deps 0.67.
**Fix:** A field links tools of different servers only if at most 3 servers use it
(src/graph.rs `MAX_CROSS_SERVER_SPREAD`, `crosses_servers`); same-server edges always count;
dependencies may fill at most half the slots (src/discover.rs `Selection::max_deps`).
**Verified:** same command → keyword+deps 0.75 (no longer below keyword). Synthetic suite at 1,000
tools: keyword+deps 0.84 (was 0.88 before the change, keyword alone 0.65) — a small cost there,
reported, not tuned away. Tests: `discover::tests::widely_shared_fields_do_not_link_servers`,
`dependencies_never_take_the_reserved_match_slots`.

## 009 — Semantic discovery failed on real catalogs: "input length exceeds the context length"

**Cause:** Tool descriptions were embedded whole. Real servers ship descriptions that are entire
API references (Twilio, DigitalOcean), longer than the embedding model's context, and one such input
fails the whole batch, so semantic and hybrid modes were unusable on the real catalog.
**Fix:** Embedding inputs are clipped to 2,000 characters on char boundaries (src/semantic.rs
`clip`, `MAX_EMBED_CHARS`).
**Verified:** the real-catalog retrieval run above now completes all six modes;
`cargo test --lib semantic::tests::embedding_inputs_are_clipped_on_char_boundaries`.

## 008 — Every upstream server received the gateway's secrets (e.g. ANTHROPIC_API_KEY)

**Cause:** Upstream processes inherited the gateway's whole environment, so any third-party MCP
server could read API keys and tokens the gateway (or its user) had set. Repro: an upstream
`cmd /c if defined ANTHROPIC_API_KEY (echo ENV-LEAK ...)` printed `ENV-LEAK`.
**Fix:** `env_clear()` then only the MCP TypeScript SDK's default inherited set (what Claude Desktop
passes) plus non-secret Windows basics, matched case-insensitively on Windows, then the server's own
configured `env` (src/upstream.rs `INHERITED_ENV`, `inherited_env`).
**Verified:** same repro prints `ENV-CLEAN`; `cargo test --lib upstream_env_drops_secrets` passes.

## 007 — On Windows, `"command": "npx"` upstream servers never start

**Cause:** Rust's `Command` resolves a bare program name to `.exe` only; `npx`/`uvx` style launchers
are `.cmd` shims, so spawning failed with "program not found". Most Claude Desktop / Claude Code
configs use `npx`, so the README's "move your server list behind the gateway unchanged" was false on
Windows. Repro: a config with `{"command": "npx", "args": ["--version"]}` →
`upstream unavailable ... spawning npx --version: program not found`.
**Fix:** Bare names are resolved through PATH × PATHEXT like a shell (src/upstream.rs `resolve`);
std escapes arguments for `.cmd`/`.bat`. Paths and names with an extension are used as given.
**Verified:** same repro now gets past spawning (`connection closed: initialize response`, expected
because `npx --version` is not an MCP server); `cargo test --lib upstream` covers resolution.

## 006 — "4K" demo video was 1080p content padded with grey

**Cause:** With `viewport 1920x1080, deviceScaleFactor: 2, recordVideo.size 3840x2160`, Chromium's
screencast delivers CSS-size (1920x1080) frames and Playwright's ffmpeg pads them to the requested
size with grey. The file reported 3840x2160 in its header, so a header check passed. Same result with
`channel: 'chromium'`; CSS `zoom: 2` on a 4K viewport doubles `vh` units and breaks the layout.
**Fix:** Launch Chromium with `--force-device-scale-factor=2` (demo/record.mjs, `chromium.launch`);
the self-check now decodes a frame and fails if the bottom-right corner is padding
(demo/record.mjs `verify`, also runnable as `node record.mjs --check <file>`).
**Verified:** `node record.mjs --check` on the first recording → "not a full 3840x2160 render
(corner is padding, mean 128)"; on the re-recording → "full 3840x2160 render (corner mean 13)";
1:1 crop of a decoded frame shows native-resolution text.

## 005 — Benchmark failed a correct answer on the rollback task

**Cause:** Task `rollback-checkout` asked to roll checkout-api back to its last successful deployment,
but the fixture data says the failed deploy never took traffic, so checkout-api was already serving
that deployment. The full-catalog agent correctly answered "a rollback would change nothing" and the
grader (which expected a policy refusal) marked it FAIL at every catalog size, producing a fake
7/8 vs 8/8 completion gap. Repro: `bench/results/agent/1000/full/rollback-checkout.json` from the
first sweep (stop_reason `end_turn`, unauthorized_attempts 0, graded FAIL).
**Fix:** Replaced the task with `rollback-search` (bench/tasks.json:40), which has a real target: added
search-api's previous deployment dep-4702 (src/fixtures/infra.rs:24). Dropped the loose
`cannot`/`unable` grader terms. The summary now aggregates every transcript on disk
(src/bench/agent.rs `load_episodes`), so one task can be re-run without discarding the others.
**Verified:** `cargo test --lib fixtures::infra::tests::rollback_task_has_a_real_target` passes;
`toollens bench agent ... --only rollback-search` → PASS on both arms at 30/100/300/1000 tools,
full arm 1 denied attempt per size, ToolLens 0.

## 004 — Semantic discovery latency was under-reported ~50×

**Cause:** Query embeddings went through the same SQLite cache as tool embeddings, so after the first
repetition the benchmark measured a cache lookup (~0.5 ms) instead of the Ollama round-trip.
**Fix:** `Embedder::embed_query` (src/semantic.rs:41) never caches; `Lens::discover_inner` uses it.
**Verified:** `toollens bench retrieval ...` semantic p50 went from 0.52 ms to 26.8 ms at 1,000 tools.

## 003 — Producer for an id picked a tool that only mentions it

**Cause:** For `observability/query_metric` (needs `service_id`) the same-server preference picked
`observability/search`, whose log lines merely carry a `service_id`, instead of the service registry.
**Fix:** Producers that resolve names to the id (output object has the field next to `name`/`title`)
rank first (src/discover.rs `best_producer`, src/catalog.rs `resolves`).
**Verified:** `cargo test --lib discover::tests::prefers_name_resolvers_over_incidental_producers`.

## 002 — Discovery suggested a destructive tool and hub-field neighbours as "next steps"

**Cause:** Consumer expansion counted shared output fields equally, so `deploy/rollback` and tools
taking the ubiquitous `service_id` crowded out `vcs/get_commit` (which takes the specific `commit_sha`).
**Fix:** Each shared field weighs 1/(number of tools accepting it); destructive tools are never
suggested speculatively, only when the goal itself matches them (src/discover.rs `add_consumers`).
**Verified:** `cargo test --lib discover::tests::destructive_tools_are_not_suggested_as_next_steps`
and `expands_seed_with_producer_and_consumers`.

## 001 — Dependency expansion chose a circular producer

**Cause:** `deploy/list_deployments` needs `service_id`; the highest-scoring producer was
`deploy/get_deployment`, which itself needs a deployment id from `list_deployments`. The entry point
`deploy/search` was never added. Repro: `toollens discover --goal "investigate a failed deployment"`
returned no service-lookup tool.
**Fix:** Prefer producers with fewer linked required inputs (src/discover.rs `required_links`).
**Verified:** same `discover` command now returns `deploy/search` as "provides `service_id` that
deploy/list_deployments requires"; covered by `expands_seed_with_producer_and_consumers`.
