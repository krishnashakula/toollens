// Records the ToolLens release demo: 3840x2160 at 60 fps, rendered frame by frame.
//
// Phase 1 (live): runs the demo tasks for real through the console: Claude, the gateway and the
// real MCP servers. Each run's events are saved with the moment they happened.
// Phase 2 (render): drives the page on a virtual clock, exactly 1/60 s per frame. JS timers,
// requestAnimationFrame, CSS animations and scrolling all follow that clock, so every frame is
// exact and the live runs play at their real pace. Frames go straight into the encoder (NVENC on an
// NVIDIA GPU, else x264).
//
// Needs: the console on the real catalog (see STORYBOARD.md) and a full ffmpeg with libx264:
//   TOOLLENS_URL=<the URL the console printed, with its #token=…>
//   FFMPEG=<path to ffmpeg>   (e.g. pip install imageio-ffmpeg; python -c "import imageio_ffmpeg as f; print(f.get_ffmpeg_exe())")
// Usage: node record.mjs              live runs + render
//        node record.mjs --reuse      render the last live runs again (no API cost)
//        node record.mjs --check FILE verify a recording
import { chromium } from 'playwright';
import { spawn, execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const BASE = process.env.TOOLLENS_URL ?? '';
const TOKEN = BASE.match(/#token=([0-9a-f]+)/)?.[1];
const OUT = fileURLToPath(new URL('./out/', import.meta.url));
const FINAL = join(OUT, 'toollens-demo-4k60.mp4');
const RUNS = join(OUT, 'live-runs.json');
const FFMPEG = process.env.FFMPEG ?? 'ffmpeg';
const FPS = 60, DT = 1000 / FPS, W = 3840, H = 2160;

const TASKS = [
  { arm: 'lens', prompt: 'What time is it in Tokyo right now?' },
  { arm: 'lens', prompt: "Text +15555550100 'Deploy finished' from +15555550199 using Twilio." },
  { arm: 'lens', prompt: 'Delete the DigitalOcean droplet web-old-01.' },
  { arm: 'full', prompt: "Text +15555550100 'Deploy finished' from +15555550199 using Twilio." },
];
const WARM = ['time', 'twilio', 'digitalocean'];

// Decoded pixels, not the container header: dimensions, frame rate, and a real (non-padded) frame.
function check(file) {
  let info = '';
  try { execFileSync(FFMPEG, ['-hide_banner', '-i', file], { stdio: ['ignore', 'ignore', 'pipe'] }); } catch (e) { info = String(e.stderr); }
  const m = info.match(/Video: .*?(\d{3,5})x(\d{3,5}).*?([\d.]+) fps/);
  if (!m || +m[1] !== W || +m[2] !== H || Math.round(+m[3]) !== FPS) throw new Error(`${file}: expected ${W}x${H} @ ${FPS} fps, got ${m ? `${m[1]}x${m[2]} @ ${m[3]} fps` : 'no video stream'}`);
  const corner = execFileSync(FFMPEG, ['-loglevel', 'error', '-ss', '2', '-i', file, '-frames:v', '1', '-vf', `crop=64:64:${W - 64}:${H - 64}`, '-f', 'rawvideo', '-pix_fmt', 'gray', '-']);
  const grey = corner.filter(v => Math.abs(v - 128) < 8).length;
  if (corner.length !== 64 * 64 || grey > 0.9 * corner.length) throw new Error(`${file}: frame is not a full render`);
  const dur = info.match(/Duration: (\d+):(\d+):([\d.]+)/);
  console.log(`ok: ${file} is ${W}x${H} @ ${FPS} fps, ${dur ? `${+dur[2] * 60 + +dur[3]}s` : '?'}, full render`);
}
if (process.argv[2] === '--check') { check(process.argv[3] ?? FINAL); process.exit(0); }

if (!TOKEN) throw new Error('set TOOLLENS_URL to the URL the console printed (http://127.0.0.1:7300/#token=…)');
async function api(path, body) {
  const headers = { 'x-toollens-token': TOKEN, ...(body && { 'content-type': 'application/json' }) };
  const r = await fetch(new URL(path, BASE), body ? { method: 'POST', headers, body: JSON.stringify(body) } : { headers });
  if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
  return r.json();
}
const sleep = ms => new Promise(r => setTimeout(r, ms));

const catalog = await api('/api/catalog').catch(e => { throw new Error(`console not reachable at ${BASE} (${e.message}); see STORYBOARD.md`); });
if (catalog.tools < 2000) console.warn(`warning: catalog has ${catalog.tools} tools; the storyboard expects the real catalog (bench/real/catalog.json)`);
try { execFileSync(FFMPEG, ['-hide_banner', '-encoders'], { stdio: 'pipe' }).toString().includes('libx264') || (() => { throw 0; })(); }
catch { throw new Error(`no ffmpeg with libx264 at ${FFMPEG}; set FFMPEG (see header)`); }
mkdirSync(OUT, { recursive: true });

// ---- Phase 1: the live runs
let runs;
if (process.argv.includes('--reuse') && existsSync(RUNS)) {
  runs = JSON.parse(readFileSync(RUNS, 'utf8'));
  console.log(`reusing ${runs.length} live runs from ${RUNS}`);
} else {
  console.log('warming', WARM.join(', '), await api('/api/warm', { servers: WARM }).then(JSON.stringify));
  runs = [];
  for (const t of TASKS) {
    const { id } = await api('/api/live', t);
    let s;
    for (let i = 0; i < 600; i++) { await sleep(500); s = await api('/api/live/' + id); if (s.done) break; }
    if (!s?.done) throw new Error(`live run ${id} did not finish`);
    const fin = s.events.at(-1);
    console.log(`live ${t.arm} "${t.prompt}" -> ${(fin.elapsed_ms / 1000).toFixed(1)}s, ${fin.requests} requests${fin.error ? ', ' + fin.error.slice(0, 80) : ''}`);
    runs.push({ id, ...t });
  }
  writeFileSync(RUNS, JSON.stringify(runs, null, 1));
}

// ---- Phase 2: frame-exact render
// GPU by default: Chromium rasterizes through ANGLE/D3D11 and NVENC encodes; --cpu forces software.
// --force_high_performance_gpu: on hybrid laptops Windows otherwise hands Chromium the integrated GPU.
const gpu = !process.argv.includes('--cpu');
const browser = await chromium.launch(gpu
  ? { channel: 'chromium', args: ['--force-device-scale-factor=2', '--force_high_performance_gpu', '--enable-gpu-rasterization', '--ignore-gpu-blocklist', '--use-angle=d3d11', '--enable-zero-copy'] }
  : { args: ['--force-device-scale-factor=2'] });
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 2, colorScheme: 'dark' });
const pageErrors = [];
page.on('pageerror', e => pageErrors.push(e.message));
await page.clock.install({ time: new Date('2026-10-26T12:00:00Z') });
await page.clock.pauseAt(new Date('2026-10-26T12:00:00.100Z'));
await page.goto(BASE, { waitUntil: 'networkidle' });
// Raw CDP capture does not hide the caret (page.screenshot does), and its blink runs on wall time.
await page.addStyleTag({ content: 'html { scroll-behavior: auto !important; } * { caret-color: transparent !important; }' });
const renderer = await page.evaluate(() => {
  const gl = document.createElement('canvas').getContext('webgl');
  const ext = gl?.getExtension('WEBGL_debug_renderer_info');
  return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : 'no WebGL';
});
// CDP capture with optimizeForSpeed: ~82 ms per 4K frame against ~134 ms through page.screenshot.
const cdp = await page.context().newCDPSession(page);

const nvenc = gpu && execFileSync(FFMPEG, ['-hide_banner', '-encoders'], { stdio: 'pipe' }).toString().includes('h264_nvenc');
const encoder = nvenc
  ? ['-c:v', 'h264_nvenc', '-preset', 'p7', '-tune', 'hq', '-rc', 'vbr', '-cq', '17', '-b:v', '0', '-profile:v', 'high']
  : ['-c:v', 'libx264', '-preset', 'medium', '-tune', 'animation', '-crf', '15', '-profile:v', 'high', '-level:v', '5.2'];
console.log(`render: ${gpu ? 'GPU raster' : 'software raster'} (${renderer}), ${nvenc ? 'NVENC' : 'x264'} encode`);
const ff = spawn(FFMPEG, ['-loglevel', 'error', '-y', '-f', 'image2pipe', '-framerate', String(FPS), '-c:v', 'mjpeg', '-i', '-',
  ...encoder, '-pix_fmt', 'yuv420p', '-r', String(FPS), '-movflags', '+faststart', FINAL], { stdio: ['pipe', 'inherit', 'inherit'] });
let frames = 0;
const started = Date.now();
// CSS animations and transitions run on the document timeline, not the JS clock: pause each one
// and set its time from the virtual clock every frame.
const stepCss = () => {
  const now = performance.now();
  for (const a of document.getAnimations()) {
    if (a.__t0 === undefined) { a.__t0 = now; a.pause(); }
    a.currentTime = now - a.__t0;
  }
};
// page.screenshot had a 30 s timeout; the raw CDP call has none, so add it back.
function capture() {
  let t;
  const stalled = new Promise((_, rej) => { t = setTimeout(() => rej(new Error(`captureScreenshot stalled at frame ${frames}`)), 30_000); });
  return Promise.race([cdp.send('Page.captureScreenshot', { format: 'jpeg', quality: 94, optimizeForSpeed: true }), stalled]).finally(() => clearTimeout(t));
}
async function frame() {
  await page.clock.runFor(DT);
  await page.evaluate(stepCss);
  const { data } = await capture();
  const jpg = Buffer.from(data, 'base64');
  if (!ff.stdin.write(jpg)) await once(ff.stdin, 'drain');
  if (++frames % 600 === 0) console.log(`  ${frames / FPS}s rendered (${((Date.now() - started) / 1000).toFixed(0)}s wall)`);
}
const hold = async ms => { for (let i = Math.round(ms / DT); i > 0; i--) await frame(); };
// Never return a page promise that settles on rAF or timers (e.g. the scroll tween): evaluate would wait
// for it, and it only settles when the virtual clock moves, which frame() cannot do meanwhile. Deadlock.
const scene = async (id, ms = 900) => { await page.evaluate(([i, m]) => { window.toollens.scrollToScene(i, m); }, [id, ms]); await hold(ms + 50); };
async function type(sel, text, cps = 48) {
  for (let i = 1; i <= text.length; i++) {
    await page.evaluate(([s, t]) => window.toollens.typeInto(s, t), [sel, text.slice(0, i)]);
    await hold(1000 / cps);
  }
}
async function untilDone(maxMs) {
  for (let t = 0; t < maxMs; t += DT) {
    await frame();
    if (frames % 6 === 0 && await page.evaluate(() => window.__liveDone === true)) return;
  }
  throw new Error('live replay did not finish');
}

// 0:00 hypothesis
await hold(3000);
// 0:03 the real catalog
await scene('catalog');
await hold(3200);
// 0:07 live runs, each at its real pace
await scene('live');
await hold(500);
for (const r of runs) {
  await page.click(`#live-arm button[data-v="${r.arm}"]`);
  await type('#live-prompt', r.prompt);
  await hold(250);
  await page.evaluate(id => window.toollens.replayLive(id), r.id);
  await untilDone(180_000);
  await hold(2600);
}
// the measured answer on all 110 servers
await scene('real');
await hold(4200);
await page.evaluate(() => { window.toollens.scrollToScene('real-charts', 900); });
await hold(5200);

ff.stdin.end();
await once(ff, 'close');
await browser.close();
if (pageErrors.length) throw new Error(`page errors during render:\n${pageErrors.join('\n')}`);
console.log(`rendered ${frames} frames (${(frames / FPS).toFixed(1)}s) in ${((Date.now() - started) / 1000).toFixed(0)}s`);
check(FINAL);
