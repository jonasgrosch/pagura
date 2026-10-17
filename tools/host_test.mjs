/** Offline parity, independent pixel oracles and browser-shim checks.
 * Prerequisites: cargo build --locked --offline -p pagura-host; cargo wasm --offline
 * Usage: node tools/host_test.mjs
 */
import assert from 'node:assert/strict';
import { readFile, mkdtemp, open, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { WASI } from 'node:wasi';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';
import { FrameDecoder, encodeRequest, HostError, decodeResponse, MAX_FILE } from '../web/protocol.mjs';
import { SessionClient } from '../web/client.mjs';
import { createWasi } from '../web/wasi.mjs';
import { runHost } from '../web/worker.mjs';
import { fixtureUrl, loadFixture } from '../web/fixtures.mjs';

if (!isMainThread) {
  try { await runHost(workerData.module, workerData.shared, (message, transfer = []) => parentPort.postMessage(message, transfer)); }
  catch (error) { parentPort.postMessage({ type: 'fatal', message: String(error) }); }
} else {
  await main();
}

function bytesEqual(actual, expected, message) {
  assert.equal(actual.length, expected.length, `${message}: byte count`);
  const first = actual.findIndex((byte, index) => byte !== expected[index]);
  assert.equal(first, -1, `${message}: first different byte ${first}`);
}
function parse(bytes) {
  const frames = [], decoder = new FrameDecoder((frame) => frames.push(frame));
  // Vary chunk boundaries through every fixed-size field and large tile.
  for (let offset = 0; offset < bytes.length; offset += 137) decoder.feed(bytes.subarray(offset, offset + 137));
  decoder.finish(); return frames;
}
function ppmPixels(bytes, width, height) {
  // Fixture generator emits canonical P6, independently of the engine.
  const header = Buffer.from(`P6\n${width} ${height}\n255\n`);
  bytesEqual(bytes.subarray(0, header.length), header, 'oracle PPM header');
  assert.equal(bytes.length - header.length, width * height * 3);
  return bytes.subarray(header.length);
}
// `tolerance` is the manifest's per-channel allowance (0 for exact oracles).
function checkPixels(tile, rgb, tolerance = 0) {
  assert.equal(tile.rgba.length, rgb.length / 3 * 4);
  for (let pixel = 0; pixel < rgb.length / 3; pixel++) {
    for (let channel = 0; channel < 3; channel++) {
      if (Math.abs(tile.rgba[pixel * 4 + channel] - rgb[pixel * 3 + channel]) > tolerance) assert.fail(`Oracle mismatch at pixel ${pixel}, channel ${channel}`);
    }
    assert.equal(tile.rgba[pixel * 4 + 3], 255);
  }
}

// host-multipage-navigation: independently stated PDF geometry, not engine pixels.
// Page 2's blue square is [32, 32, 64, 64] in bottom-left PDF coordinates.
// On a 64x64 top-left surface that covers x=[32,64), y=[0,32).
function secondPageRgb(tile) {
  const rgb = new Uint8Array(tile.width * tile.height * 3).fill(255);
  for (let y = 0; y < tile.height; y++) for (let x = 0; x < tile.width; x++) {
    if (tile.x + x >= 32 && tile.x + x < 64 && tile.y + y >= 0 && tile.y + y < 32) {
      const offset = (y * tile.width + x) * 3;
      rgb[offset] = 0; rgb[offset + 1] = 0; rgb[offset + 2] = 255;
    }
  }
  return rgb;
}

async function testTwoPages(client, root, native, wasiRun) {
  const pdf = new Uint8Array(await readFile(join(root, 'corpus/generated/classic/two-pages.pdf')));
  const tiles = [
    { x: 0, y: 0, width: 64, height: 64 },
    { x: 24, y: 16, width: 32, height: 32 },
    { x: 32, y: 0, width: 32, height: 32 },
    { x: 0, y: 32, width: 32, height: 32 },
  ];
  const input = Buffer.concat([
    encodeRequest(1, 1), encodeRequest(2, 2, { bytes: pdf, recovery: 0, password: new Uint8Array() }),
    encodeRequest(3, 3, { document: 1, page: 0 }), encodeRequest(3, 4, { document: 1, page: 1 }),
    ...tiles.map((tile, index) => encodeRequest(4, index + 5, { document: 1, page: 1, scale: 1, ...tile })),
    encodeRequest(6, 9, { document: 1 }),
  ]);
  const nativeRun = spawnSync(native, [], { input, timeout: 30000 });
  assert.ifError(nativeRun.error); assert.equal(nativeRun.status, 0);
  const wasi = await wasiRun(input, 'two-pages');
  assert.equal(wasi.code, 0); bytesEqual(wasi.bytes, nativeRun.stdout, 'two-page native/WASI parity');
  const frames = parse(wasi.bytes);
  assert.deepEqual(frames.map(({ kind }) => kind), [129, 130, 131, 131, 132, 132, 132, 132, 134]);
  assert.equal(frames[1].value.pages, 2);
  const expectedInfo = { crop: [0, 0, 64, 64], rotation: 0, userUnit: 1, width: 64, height: 64 };
  assert.deepEqual(frames[2].value, expectedInfo); assert.deepEqual(frames[3].value, expectedInfo);
  const opened = await client.open(pdf, 0); assert.equal(opened.pages, 2);
  for (const page of [0, 1]) assert.deepEqual(await client.pageInfo(opened.document, page), expectedInfo);
  for (const [index, tile] of tiles.entries()) {
    const expected = secondPageRgb(tile);
    checkPixels(frames[index + 4].value, expected);
    checkPixels(await client.renderTile(opened.document, 1, 1, tile), expected);
  }
  await client.close(opened.document);
}

async function main() {
  const root = fileURLToPath(new URL('../', import.meta.url));
  const native = join(root, 'target/debug/pagura-host');
  const wasm = await WebAssembly.compile(await readFile(join(root, 'target/wasm32-wasip1/release/pagura-host.wasm')));
  const serverChecks = spawnSync('python3', [join(root, 'web/test_server.py')], { encoding: 'utf8', timeout: 30000 });
  assert.ifError(serverChecks.error); assert.equal(serverChecks.status, 0, serverChecks.stderr);
  const manifest = JSON.parse(await readFile(join(root, 'corpus/manifests/classic.json'), 'utf8'));
  const scratch = await mkdtemp(join(tmpdir(), 'pagura-host-'));
  const files = [];
  const wasiRun = async (input, name) => {
    const inputPath = join(scratch, `${name}.in`), outputPath = join(scratch, `${name}.out`), errorPath = join(scratch, `${name}.err`);
    const source = await open(inputPath, 'w+'), destination = await open(outputPath, 'w+'), stderr = await open(errorPath, 'w+');
    files.push(source, destination, stderr);
    await source.writeFile(input); // Reset fd position after writing request bytes.
    await source.close(); files.splice(files.indexOf(source), 1);
    const stdin = await open(inputPath, 'r'); files.push(stdin);
    const wasi = new WASI({ version: 'preview1', args: [], env: {}, preopens: {}, returnOnExit: true,
      stdin: stdin.fd, stdout: destination.fd, stderr: stderr.fd });
    const instance = await WebAssembly.instantiate(wasm, { wasi_snapshot_preview1: wasi.wasiImport });
    const code = wasi.start(instance);
    return { code, bytes: await readFile(outputPath), stderr: await readFile(errorPath) };
  };
  let client;
  try {
    // Inject only worker construction. The tested session client, mailbox,
    // WASI shim, fd_read/fd_write bridge and engine are exactly the browser code.
    class TestWorker {
      constructor() { this.listeners = new Map(); }
      addEventListener(type, listener) {
        const listeners = this.listeners.get(type) ?? []; listeners.push(listener); this.listeners.set(type, listeners);
      }
      postMessage(message) {
        assert.equal(message.type, 'start');
        this.worker = new Worker(new URL(import.meta.url), { execArgv: ["--disable-warning=ExperimentalWarning"], workerData: { module: wasm, shared: message.shared } });
        this.worker.on('message', (data) => this.emit('message', { data }));
        this.worker.on('error', (error) => this.emit('error', error));
      }
      emit(type, event) { for (const listener of this.listeners.get(type) ?? []) listener(event); }
      terminate() { void this.worker?.terminate(); }
    }
    client = new SessionClient(new TestWorker()); await client.hello();
    let tested = 0;
    for (const entry of manifest.cases.filter((entry) => entry.expected === 'render')) {
      const pdf = new Uint8Array(await readFile(join(root, entry.path)));
      const tile = { x: 0, y: 0, width: entry.width, height: entry.height };
      const input = Buffer.concat([
        encodeRequest(1, 1), encodeRequest(2, 2, { bytes: pdf, password: new Uint8Array(), recovery: 1 }),
        encodeRequest(3, 3, { document: 1, page: 0 }),
        encodeRequest(4, 4, { document: 1, page: 0, scale: 1, ...tile }),
        encodeRequest(5, 5, { document: 1 }), encodeRequest(6, 6, { document: 1 }),
      ]);
      const nativeRun = spawnSync(native, [], { input, maxBuffer: 128 * 1024 * 1024, timeout: 30000 });
      assert.ifError(nativeRun.error); assert.equal(nativeRun.status, 0, nativeRun.stderr?.toString());
      const wasi = await wasiRun(input, `case-${tested}`);
      assert.equal(wasi.code, 0, wasi.stderr.toString()); bytesEqual(wasi.bytes, nativeRun.stdout, `${entry.id}: native/WASI response identity`);
      const frames = parse(wasi.bytes);
      assert.deepEqual(frames.map((frame) => frame.kind), [129, 130, 131, 132, 133, 134]);
      assert.deepEqual(frames.map((frame) => frame.id), [1, 2, 3, 4, 5, 6]);
      const rgb = ppmPixels(await readFile(join(root, entry.oracle)), entry.width, entry.height);
      checkPixels(frames[3].value, rgb, entry.tolerance ?? 0);
      const opened = await client.open(pdf);
      const info = await client.pageInfo(opened.document, 0);
      assert.equal(info.width, entry.width); assert.equal(info.height, entry.height);
      const browserTile = await client.renderTile(opened.document, 0, 1, tile);
      bytesEqual(browserTile.rgba, frames[3].value.rgba, `${entry.id}: browser shim/native pixels`);
      const diagnostics = await client.diagnostics(opened.document);
      assert.deepEqual(diagnostics, frames[4].value);
      assert.deepEqual(diagnostics.map((item) => item.tag), entry.repairs);
      // Partial device rectangles exercise the tile offset and edge dimensions.
      for (const partial of [
        { x: 0, y: 0, width: Math.min(31, entry.width), height: entry.height },
        { x: entry.width - 1, y: entry.height - 1, width: 1, height: 1 },
      ]) {
        const rendered = await client.renderTile(opened.document, 0, 1, partial);
        for (let y = 0; y < partial.height; y++) {
          const expectedStart = ((partial.y + y) * entry.width + partial.x) * 4;
          bytesEqual(rendered.rgba.subarray(y * partial.width * 4, (y + 1) * partial.width * 4),
            browserTile.rgba.subarray(expectedStart, expectedStart + partial.width * 4), `${entry.id}: partial tile row`);
        }
      }
      await client.close(opened.document);
      await assert.rejects(client.pageInfo(opened.document, 0), (error) => error instanceof HostError && error.category === 7);
      tested++;
    }
    await testTwoPages(client, root, native, wasiRun);
    // Explicit unsupported failures survive browser framing with context/tag.
    const unsupported = await client.open(new Uint8Array(await readFile(join(root, 'corpus/generated/classic/text-unsupported.pdf'))));
    await assert.rejects(client.renderTile(unsupported.document, 0, 1, { x: 0, y: 0, width: 64, height: 64 }),
      (error) => error instanceof HostError && error.category === 3 && error.tag.length > 0);
    await client.close(unsupported.document);
    const trailing = new Uint8Array(await readFile(join(root, 'corpus/generated/classic/trailing-bytes.pdf')));
    await assert.rejects(client.open(trailing, 0),
      (error) => error instanceof HostError && error.category === 1 && error.tag === '' && error.repair === 'file.trailing-bytes');
    const strictInput = Buffer.concat([encodeRequest(1, 1),
      encodeRequest(2, 2, { bytes: trailing, password: new Uint8Array(), recovery: 0 })]);
    const strictNative = spawnSync(native, [], { input: strictInput, timeout: 30000 });
    assert.ifError(strictNative.error); assert.equal(strictNative.status, 0);
    const strictWasi = await wasiRun(strictInput, 'strict-repair');
    assert.equal(strictWasi.code, 0); bytesEqual(strictWasi.bytes, strictNative.stdout, 'strict repair native/WASI parity');
    assert.equal(parse(strictWasi.bytes)[1].value.repair, 'file.trailing-bytes');
    const malformed = [
      Buffer.from([255, 255, 255, 255]), Buffer.from([1, 0]), Buffer.from([0, 0, 0, 0]),
      Buffer.from([5, 0, 0, 0, 77, 2, 0, 0, 0]),
      encodeRequest(5, 1, { document: 1 }), Buffer.concat([encodeRequest(1, 1), encodeRequest(1, 2)]),
    ];
    for (let index = 0; index < malformed.length; index++) {
      const nativeRun = spawnSync(native, [], { input: malformed[index], timeout: 30000 });
      assert.ifError(nativeRun.error); assert.equal(nativeRun.status, 1);
      const wasi = await wasiRun(malformed[index], `bad-${index}`);
      assert.equal(wasi.code, 1); bytesEqual(wasi.bytes, nativeRun.stdout, 'malformed native/WASI parity');
      assert.equal(parse(wasi.bytes).at(-1).kind, 255);
    }
    const cleanExit = new Promise((done, reject) => {
      const timer = setTimeout(() => reject(new Error('Worker failed to exit at stdin EOF')), 10000);
      client.worker.addEventListener('message', ({ data }) => {
        if (data.type === 'exit') { clearTimeout(timer); done(data.code); }
      });
    });
    client.input.end(); assert.equal(await cleanExit, 0);
    testCodecAndCapabilities();
    await testFixtureLoading();
    await testViewer(TestWorker, root, manifest);
    const startupPdf = await readFile(join(root, 'corpus/generated/classic/two-pages.pdf'));
    for (const startup of [
      { name: 'fixture', url: 'http://localhost:8000/?file=/fixtures/classic/two-pages.pdf',
        fetch: async () => new Response(startupPdf) },
      { name: 'disabled', url: 'http://localhost:8000/?file=/fixtures/classic/two-pages.pdf',
        fetch: async () => new Response('', { status: 404 }), error: /HTTP 404.*--fixtures/ },
      { name: 'external', url: 'http://localhost:8000/?file=https://example.invalid/fixtures/file.pdf',
        fetch: async () => assert.fail('External URL reached fetch'), error: /same-origin/ },
      { name: 'private', url: 'http://localhost:8000/?file=/Cargo.toml',
        fetch: async () => assert.fail('Private path reached fetch'), error: /same-origin/ },
      { name: 'duplicate', url: 'http://localhost:8000/?file=/fixtures/a.pdf&file=/fixtures/b.pdf',
        fetch: async () => assert.fail('Ambiguous startup URL reached fetch'), error: /specified once/ },
    ]) await testViewer(TestWorker, root, manifest, startup);
    console.log(`Host checks passed: ${tested} classic render cases; native/WASI bytes; browser worker shim/client pixels; ${malformed.length} terminal failures; independent PPM oracles; capability denials; refused repair tags; multi-page navigation; startup fixture loading; viewer DOM harness.`);
  } finally {
    client?.destroy();
    await Promise.all(files.map((file) => file.close())); await rm(scratch, { recursive: true, force: true });
  }
}

function testCodecAndCapabilities() {
  bytesEqual(encodeRequest(1, 7), Uint8Array.from([9, 0, 0, 0, 1, 7, 0, 0, 0, 1, 0, 0, 0]), 'literal JS handshake oracle');
  for (const bytes of [new Uint8Array([0, 0, 0, 0]), new Uint8Array([255, 255, 255, 255])]) {
    assert.throws(() => new FrameDecoder(() => {}).feed(bytes));
  }
  const decoder = new FrameDecoder(() => {}); decoder.feed(new Uint8Array([1])); assert.throws(() => decoder.finish());
  assert.throws(() => encodeRequest(4, 1, { document: 1, page: 0, scale: NaN }));
  const refused = Buffer.concat([
    Buffer.from([255, 2, 0, 0, 0, 1, 0, 0, 0, 0, 19, 0, 0, 0]),
    Buffer.from('file.trailing-bytes'), Buffer.from([3, 0, 0, 0]), Buffer.from('bad'), Buffer.from([0, 0]),
  ]);
  const failure = decodeResponse(refused).value;
  assert.equal(failure.category, 1); assert.equal(failure.tag, ''); assert.equal(failure.repair, 'file.trailing-bytes');
  assert.match(String(failure), /recovery would repair it \[file\.trailing-bytes\]/);
  const wrongCategory = refused.slice(); wrongCategory[5] = 2;
  assert.throws(() => decodeResponse(wrongCategory), /Invalid structured error/);
  const draft = Buffer.from([255, 2, 0, 0, 0, 1, 0, 0, 0, 0, 3, 0, 0, 0, 98, 97, 100, 0, 0]);
  assert.throws(() => decodeResponse(draft), /Truncated protocol response/);
  const memory = new WebAssembly.Memory({ initial: 1 });
  const wasi = createWasi(() => memory, (bytes) => { bytes.fill(7); return bytes.length; }, () => {});
  const view = new DataView(memory.buffer);
  view.setUint32(0, 64, true); view.setUint32(4, 16, true);
  assert.equal(wasi.fd_read(0, 0, 1, 32), 0); assert.equal(view.getUint32(32, true), 16);
  assert.equal(wasi.fd_read(3, 0, 1, 32), 8); assert.equal(wasi.fd_write(0, 0, 1, 32), 8);
  for (const descriptor of [0, 1, 2, 3, 99]) assert.equal(wasi.fd_prestat_get(descriptor, 0), 8);
  assert.equal(wasi.args_sizes_get(32, 36), 0); assert.equal(view.getUint32(32, true), 0); assert.equal(view.getUint32(36, true), 0);
  assert.equal(wasi.environ_sizes_get(32, 36), 0); assert.equal(view.getUint32(32, true), 0); assert.equal(view.getUint32(36, true), 0);
  assert.equal(wasi.clock_time_get(99, 0n, 32), 28); assert.equal(wasi.clock_time_get(1, 0n, 32), 0);
  assert.throws(() => wasi.fd_write(1, 65535, 1, 32), /outside linear memory/);
  assert.throws(() => wasi.path_open(), /Unsupported WASI import: wasi_snapshot_preview1.path_open/);
  assert.throws(() => wasi.sock_open(), /Unsupported WASI import/);
  assert.throws(() => wasi.proc_exit(7), (error) => error.code === 7);
  memory.grow(1); // All shim views must be rebuilt after growth detaches old views.
  assert.equal(wasi.random_get(65536, 65536), 0);
  assert.equal(wasi.clock_time_get(0, 0n, 65536), 0);
  assert.throws(() => wasi.random_get(-1, 8), /outside linear memory/);
}

/** Exercise the actual viewer module without claiming browser layout coverage.
 * Minimal DOM/Canvas stand-ins expose event and state errors, while all PDF
 * pixels still come from the real WASM command in a worker and the actual shim.
 */
async function testViewer(TestWorker, root, manifest, startup = null) {
  class Element {
    constructor(id) {
      this.id = id; this.children = []; this.listeners = new Map(); this.style = {};
      this.hidden = ['page-surface', 'error'].includes(id); this.value = id === 'zoom' ? '1' : '1';
      this.scrollTop = 0; this.scrollLeft = 0; this.checked = false; this.textContent = '';
    }
    addEventListener(type, listener) {
      const list = this.listeners.get(type) ?? []; list.push(listener); this.listeners.set(type, list);
    }
    dispatch(type, extra = {}) { for (const fn of this.listeners.get(type) ?? []) fn({ target: this, ...extra }); }
    replaceChildren(...children) { this.children = children; for (const child of children) child.parent = this; }
    append(child) { this.children.push(child); child.parent = this; }
    remove() { if (this.parent) this.parent.children = this.parent.children.filter((child) => child !== this); }
    setAttribute(name, value) { this[name] = value; }
    removeAttribute(name) { delete this[name]; }
    getBoundingClientRect() {
      if (this.id === 'viewport') return { left: 0, top: 0, right: 300, bottom: 300 };
      return { left: -elements.get('viewport').scrollLeft, top: -elements.get('viewport').scrollTop,
        right: parseFloat(this.style.width), bottom: parseFloat(this.style.height) };
    }
    getContext(kind) { assert.equal(kind, '2d'); return { putImageData: (image) => { this.image = image; } }; }
  }
  const ids = ['page-surface', 'viewport', 'status', 'error', 'page', 'zoom', 'previous', 'next', 'count',
    'file', 'strict', 'filename', 'diagnostic-count', 'repair-list', 'diagnostics'];
  const elements = new Map(ids.map((id) => [id, new Element(id)]));
  const original = new Map(['window', 'document', 'Worker', 'ImageData', 'requestAnimationFrame', 'crossOriginIsolated', 'fetch']
    .map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const fakeWindow = new Element('window'); fakeWindow.devicePixelRatio = 1;
  fakeWindow.location = new URL(startup?.url ?? 'http://localhost:8000/');
  Object.assign(globalThis, {
    window: fakeWindow,
    document: { getElementById: (id) => elements.get(id), createElement: (name) => new Element(name), body: { classList: { add() {}, remove() {} } } },
    Worker: TestWorker, crossOriginIsolated: true,
    requestAnimationFrame: (fn) => setTimeout(fn, 0),
    ImageData: class { constructor(data, width, height) { this.data = data; this.width = width; this.height = height; } },
  });
  if (startup) globalThis.fetch = startup.fetch;
  const waitFor = async (predicate) => {
    const deadline = performance.now() + 10000;
    while (!predicate()) {
      if (performance.now() >= deadline) assert.fail(`Viewer did not settle: ${elements.get('status').textContent}; ${elements.get('error').textContent}`);
      await new Promise((done) => setTimeout(done, 5));
    }
  };
  const pick = async (entry) => {
    const picker = elements.get('file'); picker.files = [new File([await readFile(join(root, entry.path))], entry.id + '.pdf')];
    picker.dispatch('change');
  };
  const ready = () => elements.get('status').textContent === 'Visible tiles rendered';
  try {
    await import(`../web/viewer.mjs?host-test=${startup?.name ?? 'default'}`);
    if (startup) {
      if (startup.error) {
        assert.equal(elements.get('error').hidden, false);
        assert.match(elements.get('error').textContent, startup.error);
        assert.equal(elements.get('page-surface').children.length, 0);
      } else {
        await waitFor(ready);
        assert.equal(elements.get('filename').textContent, 'two-pages.pdf');
        assert.equal(elements.get('count').textContent, 'of 2');
        const first = manifest.cases.find((entry) => entry.id === 'classic/two-pages');
        checkPixels({ rgba: elements.get('page-surface').children[0].image.data },
          ppmPixels(await readFile(join(root, first.oracle)), 64, 64));
      }
      return;
    }
    assert.equal(elements.get('status').textContent, 'Ready — open a PDF');
    for (const entry of manifest.cases.filter((entry) => entry.expected === 'render')) {
      await pick(entry); await waitFor(ready);
      assert.equal(elements.get('error').hidden, true);
      assert.equal(elements.get('diagnostic-count').textContent, `Repairs: ${entry.repairs.length}`);
      assert.equal(elements.get('page').value, '1');
      const rgb = ppmPixels(await readFile(join(root, entry.oracle)), entry.width, entry.height);
      for (const canvas of elements.get('page-surface').children) {
        const x = parseFloat(canvas.style.left), y = parseFloat(canvas.style.top), rgba = canvas.image.data;
        for (let row = 0; row < canvas.height; row++) for (let column = 0; column < canvas.width; column++) {
          const pixel = (row * canvas.width + column) * 4, oracle = ((y + row) * entry.width + x + column) * 3;
          const tolerance = entry.tolerance ?? 0;
          const differs = [0, 1, 2].some((channel) => Math.abs(rgba[pixel + channel] - rgb[oracle + channel]) > tolerance);
          if (differs || rgba[pixel + 3] !== 255) {
            assert.fail(`Viewer canvas mismatch: ${entry.id}, (${x + column}, ${y + row})`);
          }
        }
      }
      if (entry.id === 'classic/many-rectangles') {
        const viewport = elements.get('viewport'), canvases = () => elements.get('page-surface').children;
        viewport.scrollTop = 400; viewport.dispatch('scroll');
        await waitFor(() => canvases().length === 2 && canvases().every((canvas) => parseFloat(canvas.style.top) === 256) && ready());
        viewport.scrollTop = 0; viewport.dispatch('scroll');
        await waitFor(() => canvases().length === 4 && ready());
      }
    }
    const twoPages = manifest.cases.find((entry) => entry.id === 'classic/two-pages');
    await pick(twoPages); await waitFor(ready);
    const firstPage = ppmPixels(await readFile(join(root, twoPages.oracle)), 64, 64);
    const checkPage = (index) => {
      assert.equal(elements.get('page').value, String(index + 1));
      assert.equal(elements.get('count').textContent, 'of 2');
      assert.equal(elements.get('previous').disabled, index === 0);
      assert.equal(elements.get('next').disabled, index === 1);
      assert.equal(elements.get('page-surface').children.length, 1);
      const canvas = elements.get('page-surface').children[0];
      checkPixels({ rgba: canvas.image.data }, index === 0 ? firstPage : secondPageRgb({ x: 0, y: 0, width: 64, height: 64 }));
    };
    checkPage(0);
    elements.get('next').dispatch('click'); await waitFor(ready); checkPage(1);
    elements.get('previous').dispatch('click'); await waitFor(ready); checkPage(0);
    for (const index of [1, 0, 1]) {
      elements.get('page').value = String(index + 1); elements.get('page').dispatch('change');
      await waitFor(ready); checkPage(index);
    }
    const rectangle = manifest.cases.find((entry) => entry.id === 'classic/rectangle');
    await pick(rectangle); await waitFor(ready);
    elements.get('zoom').value = '2'; elements.get('zoom').dispatch('change');
    elements.get('zoom').value = '1'; elements.get('zoom').dispatch('change');
    await waitFor(ready); assert.equal(elements.get('page-surface').style.width, '64px');
    fakeWindow.devicePixelRatio = 2; fakeWindow.dispatch('resize');
    await waitFor(ready); assert.equal(elements.get('page-surface').children[0].width, 128);
    assert.equal(elements.get('page-surface').style.width, '64px');
    fakeWindow.devicePixelRatio = 1; fakeWindow.dispatch('resize'); await waitFor(ready);
    // Unsupported content clears any partial tiles and names the failure.
    await pick(manifest.cases.find((entry) => entry.id === 'classic/text-unsupported'));
    await waitFor(() => !elements.get('error').hidden);
    assert.match(elements.get('error').textContent, /Unsupported \[[^\]]+\]/);
    assert.equal(elements.get('page-surface').children.length, 0);
    // Drop uses the same open path and releases a failed worker session.
    fakeWindow.dispatch('drop', { preventDefault() {}, dataTransfer: { files: [new File([await readFile(join(root, rectangle.path))], 'dropped.pdf')] } });
    await waitFor(ready); assert.equal(elements.get('filename').textContent, 'dropped.pdf');
    elements.get('page').value = '2'; elements.get('page').dispatch('change');
    assert.equal(elements.get('page').value, '1');
    elements.get('strict').checked = true;
    await pick(manifest.cases.find((entry) => entry.id === 'classic/trailing-bytes'));
    await waitFor(() => !elements.get('error').hidden);
    assert.match(elements.get('error').textContent, /Syntax.*recovery would repair it \[file\.trailing-bytes\]/);
    assert.equal(elements.get('page').disabled, true);
  } finally {
    fakeWindow.dispatch('pagehide');
    for (const [key, descriptor] of original) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

async function testFixtureLoading() {
  const base = 'http://localhost:8000/';
  assert.equal(fixtureUrl('/fixtures/classic/two-pages.pdf', base).href, `${base}fixtures/classic/two-pages.pdf`);
  assert.equal(fixtureUrl(`${base}fixtures/classic/two-pages.pdf`, base).origin, new URL(base).origin);
  const invalid = ['', '/viewer.mjs', '/fixtures-other/file.pdf', '/fixtures/',
    'https://example.invalid/fixtures/file.pdf', '//example.invalid/fixtures/file.pdf',
    'file:///fixtures/file.pdf', 'data:application/pdf,test', 'javascript:alert(1)',
    '/fixtures/../Cargo.toml', '/fixtures/classic/../two-pages.pdf',
    '/fixtures/%2e%2e/file.pdf', '/fixtures/classic/%2e%2e%2f..%2fsecret.pdf',
    '/fixtures/classic/%5cfile.pdf', '/fixtures/%00.pdf',
    '/fixtures/file.pdf?query=1', '/fixtures/file.pdf#fragment',
    'http://user:password@localhost:8000/fixtures/file.pdf', ' /fixtures/file.pdf'];
  const originalFetch = globalThis.fetch;
  try {
    let requests = 0;
    globalThis.fetch = async () => { requests++; assert.fail('Invalid startup URL reached fetch'); };
    for (const value of invalid) {
      assert.throws(() => fixtureUrl(value, base)); await assert.rejects(loadFixture(value, base));
    }
    assert.equal(requests, 0);
    globalThis.fetch = async (url, options) => {
      assert.equal(url, `${base}fixtures/classic/two-pages.pdf`);
      assert.deepEqual(options, { mode: 'same-origin', credentials: 'omit', redirect: 'error' });
      return new Response(Uint8Array.from([37, 80, 68, 70]), { headers: { 'Content-Length': '4' } });
    };
    const file = await loadFixture('/fixtures/classic/two-pages.pdf', base);
    assert.equal(file.name, 'two-pages.pdf'); bytesEqual(new Uint8Array(await file.arrayBuffer()), [37, 80, 68, 70], 'fixture loader bytes');
    globalThis.fetch = async () => new Response('', { status: 404 });
    await assert.rejects(loadFixture('/fixtures/classic/two-pages.pdf', base), /HTTP 404.*--fixtures/);
    globalThis.fetch = async () => new Response('small', { headers: { 'Content-Length': String(MAX_FILE + 1) } });
    await assert.rejects(loadFixture('/fixtures/classic/two-pages.pdf', base), /input budget/);
    let cancelled = false, released = false;
    globalThis.fetch = async () => ({
      ok: true, headers: new Headers(), body: { getReader: () => ({
        // Model an over-budget stream chunk without allocating 512 MiB.
        read: async () => ({ done: false, value: { byteLength: MAX_FILE + 1 } }),
        cancel: async () => { cancelled = true; }, releaseLock: () => { released = true; },
      }) },
    });
    await assert.rejects(loadFixture('/fixtures/classic/two-pages.pdf', base), /input budget/);
    assert.equal(cancelled, true); assert.equal(released, true);
  } finally { globalThis.fetch = originalFetch; }
}
