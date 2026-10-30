/** Loopback demo server and isolated hardware-browser runner. Node std only. */
import {createServer} from 'node:http';
import {readFile, readdir, realpath, mkdir, writeFile, mkdtemp, rm} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {resolve, dirname, extname, relative, sep} from 'node:path';
import {spawn} from 'node:child_process';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createHash} from 'node:crypto';
import {tmpdir} from 'node:os';

const here = dirname(fileURLToPath(import.meta.url)), root = resolve(here, '../..');
const mode = process.argv[2] ?? 'serve';
const option = (key, fallback) => { const i = process.argv.indexOf(key); return i < 0 ? fallback : process.argv[i + 1]; };
const wasm = resolve(option('--wasm', resolve(root, 'target/gpu-demo/wasm32-wasip1/release/pagura-gpu-demo.wasm')));
const pdfPaths = [];
for (let i = 3; i < process.argv.length; i++) if (process.argv[i] === '--pdf') pdfPaths.push(resolve(process.argv[++i]));
const mime = {'.mjs': 'text/javascript', '.js': 'text/javascript', '.html': 'text/html', '.css': 'text/css', '.wgsl': 'text/plain', '.wasm': 'application/wasm', '.pdf': 'application/pdf'};
const webFiles = new Set(['host_runtime.mjs', 'wasi.mjs', 'transport.mjs', 'failure.mjs']);
const demoFiles = new Set(['index.html', 'style.css', 'app.mjs', 'engine.mjs', 'gpu.mjs', 'worker.mjs', 'samples.mjs', 'measure.mjs', 'composite.wgsl', 'raster.wgsl', 'area.wgsl', 'present.wgsl', 'present.mjs', 'world.wgsl', 'world.mjs', 'world-gpu.mjs', 'world-checks.mjs', 'integrated.mjs', 'integrated-panel.mjs', 'display-study.mjs', 'display-browser.mjs', 'display-panel.mjs', 'display-tests.mjs', 'raster.mjs', 'coverage.mjs', 'retained.mjs', 'retained-study.mjs', 'retained-zoom.mjs', 'retained-checks.mjs', 'retained-tests.mjs', 'checks.mjs', 'svg.mjs', 'zoom.mjs']);
const fixtureIndex = [];
for (const name of (await readdir(resolve(root, 'corpus/manifests'))).sort()) {
  if (!name.endsWith('.json')) continue;
  const manifest = JSON.parse(await readFile(resolve(root, 'corpus/manifests', name), 'utf8'));
  for (const c of manifest.cases) if (c.expected === 'render' && !c.password) fixtureIndex.push({id: c.id, path: '/' + c.path, dpi: c.dpi ?? 72, width: c.width, height: c.height});
}
const fixtureRoot = await realpath(resolve(root, 'corpus/generated'));
const server = createServer(async (request, response) => {
  response.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
  response.setHeader('Cross-Origin-Embedder-Policy', 'require-corp');
  response.setHeader('Cross-Origin-Resource-Policy', 'same-origin');
  response.setHeader('X-Content-Type-Options', 'nosniff');
  response.setHeader('Cache-Control', 'no-store');
  if (!['GET', 'HEAD'].includes(request.method)) { response.writeHead(405); response.end(); return; }
  try {
    const path = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
    let file, data;
    if (path === '/engine.wasm') file = wasm;
    else if (path === '/fixture-index.json') data = JSON.stringify(fixtureIndex);
    else if (path === '/benchmark-inputs.json') data = JSON.stringify(pdfPaths.map((p, i) => ({label: `Document ${i + 1}`, path: `/local/${i}.pdf`})));
    else if (/^\/local\/\d+\.pdf$/.test(path)) file = pdfPaths[Number(path.slice(7, -4))];
    else if (path.startsWith('/web/') && webFiles.has(path.slice(5))) file = resolve(root, 'web', path.slice(5));
    else if (path.startsWith('/corpus/generated/')) {
      const candidate = await realpath(resolve(root, path.slice(1)));
      if (candidate.startsWith(fixtureRoot + sep) && extname(candidate) === '.pdf') file = candidate;
    } else if (path === '/' || path === '/index.html') file = resolve(here, 'index.html');
    else if (demoFiles.has(path.slice(1))) file = resolve(here, path.slice(1));
    if (data === undefined && file) data = await readFile(file);
    if (data === undefined) { response.writeHead(404); response.end('Not found'); return; }
    response.setHeader('Content-Type', path.endsWith('.json') ? 'application/json' : mime[extname(file ?? path)] ?? 'application/octet-stream');
    response.writeHead(200); response.end(request.method === 'HEAD' ? undefined : data);
  } catch (error) { response.writeHead(404); response.end(String(error)); }
});
await new Promise(resolve => server.listen(Number(option('--port', mode === 'serve' ? '8097' : '0')), '127.0.0.1', resolve));
const url = `http://127.0.0.1:${server.address().port}/`;
if (mode === 'serve') {
  console.log(`GPU demo: ${url}`);
  console.log(`WASM: ${relative(root, wasm)}`);
} else {
  const output = resolve(option('--output', resolve(root, `target/gpu-demo/${mode}.json`)));
  await mkdir(dirname(output), {recursive: true});
  const profile = await mkdtemp(resolve(tmpdir(), 'pagura-gpu-'));
  const chromePath = option('--chrome', process.env.CHROME_BIN ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome');
  const digest = bytes => createHash('sha256').update(bytes).digest('hex');
  const sourceFiles = [...demoFiles].map(file => `benchmarks/gpu-demo/${file}`).concat([
    'crates/pagura-paint/src/prepare/gpu.rs', 'crates/pagura-paint/src/prepare/gpu/tests.rs', 'crates/pagura-paint/src/prepare/gpu_raster.rs', 'crates/pagura-paint/src/prepare/gpu_raster/shared.rs', 'crates/pagura-paint/src/prepare/gpu_world.rs', 'crates/pagura-paint/src/prepare/gpu_world/geometry.rs', 'crates/pagura-paint/src/prepare/gpu_world/patches.rs', 'crates/pagura-paint/src/prepare/gpu_world/tests.rs', 'crates/pagura-paint/src/execute/tests.rs', 'crates/pagura-paint/src/plan.rs', 'crates/pagura-paint/src/prepare/planner/runs.rs', 'benchmarks/gpu-demo/engine/src/world.rs', 'benchmarks/gpu-demo/engine/src/cache.rs', 'benchmarks/gpu-demo/engine/src/tests.rs', 'crates/pagura-raster/src/band/gpu.rs', 'crates/pagura-raster/src/band/gpu/corrections.rs', 'crates/pagura-raster/Cargo.toml', 'crates/pagura-paint/Cargo.toml', 'crates/pagura-raster/src/lib.rs', 'crates/pagura-raster/src/band.rs', 'crates/pagura-paint/src/prepare.rs', 'crates/pagura-paint/src/prepare/shapes.rs',
    'benchmarks/gpu-demo/engine/Cargo.toml', 'benchmarks/gpu-demo/engine/Cargo.lock',
    'benchmarks/gpu-demo/engine/src/main.rs', 'benchmarks/gpu-demo/engine/src/svg.rs',
    'benchmarks/gpu-demo/engine/src/svg/images.rs', 'benchmarks/gpu-demo/engine/src/svg/clips.rs', 'benchmarks/gpu-demo/engine/src/svg/tests.rs',
    'benchmarks/gpu-demo/policy.json', 'web/host_runtime.mjs', 'web/transport.mjs', 'web/wasi.mjs']);
  const initialHashes = Object.fromEntries(await Promise.all(sourceFiles.map(async file => [file, digest(await readFile(resolve(root, file)))])));
  const initialWasmHash = digest(await readFile(wasm));
  const initialCommit = (await promisify(execFile)('git', ['rev-parse', 'HEAD'], {cwd: root})).stdout.trim();
  const log = [];
  const chrome = spawn(chromePath, ['--headless', '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
    '--enable-webgpu-developer-features', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], {stdio: ['ignore', 'ignore', 'pipe']});
  let socket, nextId = 0;
  const pending = new Map();
  try {
    const endpoint = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Chrome did not expose CDP')), 15000);
      chrome.once('error', reject);
      chrome.stderr.on('data', chunk => {
        log.push(String(chunk));
        const match = log.join('').match(/DevTools listening on (ws:\/\/[^\s]+)/);
        if (match) { clearTimeout(timer); resolve(new URL(match[1]).origin.replace('ws:', 'http:')); }
      });
    });
    const target = await (await fetch(`${endpoint}/json/new?${encodeURIComponent(url + '?runner=1')}`, {method: 'PUT'})).json();
    socket = new WebSocket(target.webSocketDebuggerUrl);
    socket.addEventListener('message', ({data}) => {
      const message = JSON.parse(data), request = pending.get(message.id);
      if (request) { clearTimeout(request.timer); pending.delete(message.id); message.error ? request.reject(new Error(message.error.message)) : request.resolve(message.result); }
      if (message.method === 'Runtime.consoleAPICalled') console.log(message.params.args.map(arg => arg.value ?? arg.description).join(' '));
      if (message.method === 'Runtime.exceptionThrown') log.push(JSON.stringify(message.params));
    });
    await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, {once: true}); socket.addEventListener('error', reject, {once: true}); });
    const command = (method, params = {}) => new Promise((resolve, reject) => {
      const id = ++nextId, timer = setTimeout(() => {pending.delete(id); reject(new Error(`CDP timeout: ${method}`));}, 1800000);
      pending.set(id, {resolve, reject, timer}); socket.send(JSON.stringify({id, method, params}));
    });
    await command('Runtime.enable');
    await command('Page.enable');
    await command('Emulation.setDeviceMetricsOverride', {width: 1440, height: 1100, deviceScaleFactor: 1, mobile: false});
    const loaded = await command('Runtime.evaluate', {expression: `new Promise(resolve => {if(document.readyState === 'complete') resolve(); else addEventListener('load',resolve,{once:true});})`, awaitPromise: true});
    if (loaded.exceptionDetails) throw new Error(loaded.exceptionDetails.text);
    let expression;
    if (mode === 'probe') expression = `(async()=>{const {GpuBackend}=await import('/gpu.mjs'); const gpu=await GpuBackend.create(); const info=gpu.info;gpu.destroy();return info;})()`;
    else if ((mode === 'integrated' || mode === 'integrated-ui-test')) expression = `(async()=>{const {run}=await import('/integrated.mjs');return run(${JSON.stringify({mode,samples:Number(option('--samples','100')),warmups:Number(option('--warmups','5')),coldSamples:Number(option('--cold-samples','20')),reciprocal:process.argv.includes('--world-reciprocal')})});})()`;
    else if (mode.startsWith('world')) expression = `(async()=>{const {run}=await import('/world-checks.mjs');return run(${JSON.stringify({mode,reciprocal:process.argv.includes('--world-reciprocal'),samples:Number(option('--samples','100')),warmups:Number(option('--warmups','5')),dpis:option('--dpis','72,150,300').split(',').map(Number),zoom:!process.argv.includes('--skip-zoom')})});})()`;
    else if (mode.startsWith('retained')) expression = `(async()=>{const {run}=await import('/retained-checks.mjs');return run(${JSON.stringify({mode,reciprocal:process.argv.includes('--world-reciprocal'),samples:Number(option('--samples','100')),warmups:Number(option('--warmups','5')),dpis:option('--dpis','72,150,300').split(',').map(Number),zoom:!process.argv.includes('--skip-zoom')})});})()`;
    else if (mode.startsWith('coverage')) expression = `(async()=>{const {run}=await import('/coverage.mjs');return run(${JSON.stringify({mode, samples:Number(option('--samples','100')), warmups:Number(option('--warmups','5')), dpis:option('--dpis','72,150,300').split(',').map(Number)})});})()`;
    else expression = `(async()=>{const {run}=await import('/checks.mjs');return run(${JSON.stringify({mode, samples: Number(option('--samples', '100')), warmups: Number(option('--warmups', '5')), dpis: option('--dpis', '72,150,300').split(',').map(Number), zoom: !process.argv.includes('--skip-zoom'), realOnly: process.argv.includes('--real-only'), zoomSamples: Number(option('--zoom-samples', '18')), gestureFrames: Number(option('--gesture-frames', '60'))})});})()`;
    const result = await command('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true});
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    const report = result.result.value;
    const exec = promisify(execFile);
    const source_hashes = {};
    if (report.artifacts) {
      const folder = output.replace(/\.json$/, '') + '-visuals'; await mkdir(folder, {recursive: true});
      for (const artifact of report.artifacts) {
        if (!/^[a-z0-9-]+\.(png|svg)$/.test(artifact.name)) throw new Error('Invalid visual artifact name');
        const bytes = artifact.text !== undefined ? Buffer.from(artifact.text) : Buffer.from(artifact.data_url.split(',')[1], 'base64');
        await writeFile(resolve(folder, artifact.name), bytes);
        artifact.path = relative(root, resolve(folder, artifact.name)); artifact.sha256 = digest(bytes);
        delete artifact.text; delete artifact.data_url;
      }
    }
    for (const file of [...demoFiles].sort()) source_hashes[`benchmarks/gpu-demo/${file}`] = digest(await readFile(resolve(here, file)));
    for (const file of ['crates/pagura-paint/src/prepare/gpu.rs', 'crates/pagura-paint/src/prepare/gpu/tests.rs', 'crates/pagura-paint/src/prepare/gpu_raster.rs', 'crates/pagura-paint/src/prepare/gpu_raster/shared.rs', 'crates/pagura-paint/src/prepare/gpu_world.rs', 'crates/pagura-paint/src/prepare/gpu_world/geometry.rs', 'crates/pagura-paint/src/prepare/gpu_world/patches.rs', 'crates/pagura-paint/src/prepare/gpu_world/tests.rs', 'crates/pagura-paint/src/execute/tests.rs', 'crates/pagura-paint/src/plan.rs', 'crates/pagura-paint/src/prepare/planner/runs.rs', 'benchmarks/gpu-demo/engine/src/world.rs', 'benchmarks/gpu-demo/engine/src/cache.rs', 'benchmarks/gpu-demo/engine/src/tests.rs', 'crates/pagura-raster/src/band/gpu.rs', 'crates/pagura-raster/src/band/gpu/corrections.rs', 'crates/pagura-raster/Cargo.toml', 'crates/pagura-paint/Cargo.toml', 'crates/pagura-raster/src/lib.rs', 'crates/pagura-raster/src/band.rs', 'crates/pagura-paint/src/prepare.rs', 'crates/pagura-paint/src/prepare/shapes.rs',
      'benchmarks/gpu-demo/engine/Cargo.toml', 'benchmarks/gpu-demo/engine/Cargo.lock', 'benchmarks/gpu-demo/engine/src/main.rs',
      'benchmarks/gpu-demo/engine/src/svg.rs', 'benchmarks/gpu-demo/engine/src/svg/images.rs', 'benchmarks/gpu-demo/engine/src/svg/clips.rs', 'benchmarks/gpu-demo/engine/src/svg/tests.rs',
      'benchmarks/gpu-demo/policy.json', 'web/host_runtime.mjs', 'web/transport.mjs', 'web/wasi.mjs']) source_hashes[file] = digest(await readFile(resolve(root, file)));
    report.build = {commit: initialCommit,
      rustc: (await exec('rustc', ['--version'], {cwd: root})).stdout.trim(),
      node: process.version, platform: process.platform, arch: process.arch,
      wasm_sha256: initialWasmHash, source_hashes: initialHashes,
      sources_unchanged: Object.entries(source_hashes).every(([file, hash]) => initialHashes[file] === hash) && initialWasmHash === digest(await readFile(wasm))};
    if (!report.build.sources_unchanged) {report.failures ??= [];report.failures.push({error: 'Benchmark sources or WASM changed during the run'});}
    await writeFile(output, JSON.stringify(report, null, 2) + '\n');
    console.log(JSON.stringify((mode === 'bench' || mode === 'coverage-bench' || mode === 'retained-bench' || mode === 'world-bench') ? report.summary : ['test', 'svg-test', 'coverage-test', 'retained-test', 'world-test'].includes(mode) ? {
      fixtures: report.fixtures, admitted: report.admitted, gpu_variants: report.gpu_variants,
      fallback: report.fallback ?? report.unsupported, failures: report.failures, kernel_checks: report.kernel_checks,
    } : report, null, 2));
    console.log(`Evidence: ${output}`);
    if (report.failures?.length) process.exitCode = 1;
    if (option('--screenshot', null)) {
      const {cssContentSize: size} = await command('Page.getLayoutMetrics');
      const screenshot = await command('Page.captureScreenshot', {format: 'png', captureBeyondViewport: true,
        clip: {x: 0, y: 0, width: size.width, height: size.height, scale: 1}});
      await writeFile(resolve(option('--screenshot', '')), Buffer.from(screenshot.data, 'base64'));
    }
  } finally {
    socket?.close();
    for (const request of pending.values()) clearTimeout(request.timer);
    chrome.kill('SIGTERM');
    await new Promise(resolve => { if (chrome.exitCode !== null) resolve(); else {const timer = setTimeout(() => {chrome.kill('SIGKILL');resolve();}, 5000);chrome.once('exit', () => {clearTimeout(timer);resolve();});} });
    // Chrome's detached crash reporter can retain its inherited stderr pipe.
    chrome.stderr.destroy();
    await writeFile(output + '.chrome.log', log.join(''));
    await rm(profile, {recursive: true, force: true, maxRetries: 3});
    server.close();
  }
}
