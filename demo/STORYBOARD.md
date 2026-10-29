# ToolLens demo storyboard

The release video is `demo/out/toollens-demo-4k60.mp4`: 3840×2160, 60 fps, H.264, about 75 seconds.
`demo/record.mjs` makes it in two phases:

1. **Live.** The demo tasks run for real through the console. Claude Opus 5.5 calls the gateway,
   and the gateway calls the real MCP servers in their Docker sandboxes. Every step is saved with
   the moment it happened.
2. **Render.** The page runs on a virtual clock, advanced exactly 1/60 s per frame. JS timers,
   `requestAnimationFrame`, CSS animations and scrolling all follow that clock, and one 4K
   screenshot is taken per frame and piped into the encoder: NVENC on an NVIDIA GPU, with Chromium
   rasterizing on that GPU (`--force_high_performance_gpu`, or hybrid laptops hand it the integrated
   one); otherwise x264. `--cpu` forces software raster and x264. The live runs play back at the pace
   they really ran.

The video says so on screen: "Live run · <time> · real model and MCP server calls, each step at the
moment it happened".

Why not a screen recording: Playwright's 4K video capture throttles the page to about 12 frames per
second, which reads as judder or slow motion (BUGS.md 012).

| Time | Scene | What the viewer sees |
|---|---|---|
| 0:00 | **Hypothesis** | Hero over the real catalog: 2,468 tools, 110 servers, every tool one dot |
| 0:03 | **Catalog** | The 110 vendor servers as chips, largest first, and the names that collide across servers |
| 0:07 | **Live: a real answer** | "What time is it in Tokyo right now?" ToolLens finds `time/get_current_time` among 2,468 tools in about 3 ms, the server answers, Claude replies |
| 0:17 | **Live: a real vendor** | "Text … using Twilio." `TwilioApiV2010--CreateMessage` is found, inspected and called; Twilio's real API rejects the dummy key |
| 0:30 | **Live: permission** | "Delete the DigitalOcean droplet web-old-01." The tool is found, but its server marks it destructive, so the policy denies it. Discovery is not permission. |
| 0:45 | **Live: without ToolLens** | The same Twilio task with every tool sent: 2,453 definitions, 1.43M tokens; the API rejects the request |
| 1:00 | **Real world** | 28-task results on all 110 servers: full catalog rejected, ToolLens vs Anthropic's tool search, tokens and cost per task |

Each scene shows a live elapsed timer, the tools in the model's view, model requests, input tokens
and the gateway's own audit feed.

## Re-record

```powershell
cd toollens
cargo build --release
docker build -t toollens-sandbox:1 -f bench/real/sandbox.Dockerfile bench/real    # once
./target/release/toollens console --config bench/real/catalog.json --db bench/real/.toollens.db   # leave running
$env:TOOLLENS_URL = "<the URL it printed, http://127.0.0.1:7300/#token=…>"
cd demo; npm ci; npx playwright install chromium
pip install imageio-ffmpeg     # a full ffmpeg with libx264
$env:FFMPEG = python -c "import imageio_ffmpeg as f; print(f.get_ffmpeg_exe())"
node record.mjs            # live runs (about $0.15 of API usage) + render (about 7–8 minutes on an RTX 4050)
node record.mjs --reuse    # render the saved runs again, no API cost
node record.mjs --check out/toollens-demo-4k60.mp4
```
