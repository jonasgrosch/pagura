import { externalUrl, activateTarget, targetLabel } from '../web/link_actions.mjs';
import { textLayer, selectedText } from '../web/text_layer.mjs';
import { pagePoint } from '../web/page_geometry.mjs';
import { destinationView } from '../web/destinations.mjs';
import { readingWorkload } from '../web/reading_workload.mjs';
import { OutlineTree } from '../web/outline_tree.mjs';
import { ReadingView } from '../web/reading_view.mjs';
import { navigationDocument } from '../web/navigation_document.mjs';
/** Offline parity, independent pixel oracles and browser-shim checks, as node:test cases.
 * Prerequisites: cargo build --locked --offline -p pagura-host; cargo wasm --offline
 * Usage: node --test tools/host_test.ts [--test-name-pattern=REGEXP]
 * PAGURA_PYTHON names the interpreter for web/test_server.py; tools/check.py passes its own.
 */
import assert from 'node:assert/strict';
import { readFile, mkdtemp, open, rm } from 'node:fs/promises';
import type { FileHandle } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { after, before, describe, test } from 'node:test';
import { WASI } from 'node:wasi';
import { Worker } from 'node:worker_threads';
import {
  FrameDecoder,
  encodeRequest,
  HostError,
  decodeResponse,
  MAX_FILE,
  VERSION_1_1,
  VERSION_1_2,
  VERSION_1_3,
  VERSION_1_4,
} from '../web/protocol.mjs';
import type { Diagnostic, Frame, PageInfo, RequestValues, Tile, TileRect } from '../web/protocol.mjs';
import type { Link, Match, OutlineItem, SearchResult, Target, UriAction } from '../web/reading_protocol.mjs';
import type { ReadingState } from '../web/reading_view.mjs';
import { HostFailure, failureReason } from '../web/failure.mjs';
import { SessionClient } from '../web/client.mjs';
import { createWasi } from '../web/wasi.mjs';
import { PageLayout } from '../web/layout.mjs';
import { fixtureUrl, loadFixture } from '../web/fixtures.mjs';

/** One case of corpus/manifests/<area>.json. */
interface ManifestCase {
  id: string;
  path: string;
  expected: string;
  width: number;
  height: number;
  oracle: string;
  tolerance?: number;
  repairs: string[];
  information?: string[];
  feature?: string;
  password?: string;
  files: Record<string, { path: string }>;
}
interface Manifest {
  cases: ManifestCase[];
}
/** A decoded frame whose value the assertions read by response kind. */
// biome-ignore lint/suspicious/noExplicitAny: each assertion knows the kind it inspects.
type AnyFrame = Frame & { value: any };
/** A request kind and its arguments, encoded with consecutive identifiers. */
type RequestTuple = [number, RequestValues?];
type WasiRun = (input: Uint8Array, name: string) => Promise<{ code: number; bytes: Buffer; stderr: Buffer }>;
type Listener = (event: unknown) => void;
interface Startup {
  name: string;
  url: string;
  fetch: () => Promise<Response>;
  error?: RegExp;
}

const root = fileURLToPath(new URL('../', import.meta.url));
const native = join(root, 'target/debug/pagura-host');
// tools/check.py passes the interpreter it runs under; Windows installs `python`, not `python3`.
const python = process.env.PAGURA_PYTHON || (process.platform === 'win32' ? 'python' : 'python3');
let wasm: WebAssembly.Module;
let manifest: Manifest;
let scratch: string | undefined;
const files: FileHandle[] = [];
/** The long-lived browser-shim session shared by the parity tests. */
let client: SessionClient;

const wasiRun: WasiRun = async (input, name) => {
  const inputPath = join(scratch!, `${name}.in`),
    outputPath = join(scratch!, `${name}.out`),
    errorPath = join(scratch!, `${name}.err`);
  const source = await open(inputPath, 'w+'),
    destination = await open(outputPath, 'w+'),
    stderr = await open(errorPath, 'w+');
  files.push(source, destination, stderr);
  await source.writeFile(input); // Reset fd position after writing request bytes.
  await source.close();
  files.splice(files.indexOf(source), 1);
  const stdin = await open(inputPath, 'r');
  files.push(stdin);
  const wasi = new WASI({
    version: 'preview1',
    args: [],
    env: {},
    preopens: {},
    returnOnExit: true,
    stdin: stdin.fd,
    stdout: destination.fd,
    stderr: stderr.fd,
  });
  const instance = await WebAssembly.instantiate(wasm, { wasi_snapshot_preview1: wasi.wasiImport });
  const code = wasi.start(instance);
  return { code, bytes: await readFile(outputPath), stderr: await readFile(errorPath) };
};

// Inject only worker construction. The tested session client, mailbox,
// WASI shim, fd_read/fd_write bridge and engine are exactly the browser code.
class TestWorker {
  declare listeners: Map<string, Listener[]>;
  declare worker?: Worker;
  constructor() {
    this.listeners = new Map();
  }
  addEventListener(type: string, listener: Listener) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }
  postMessage(message: { type: string; shared: ArrayBufferLike }) {
    assert.equal(message.type, 'start');
    this.worker = new Worker(new URL('./host_worker.ts', import.meta.url), {
      execArgv: ['--disable-warning=ExperimentalWarning'],
      workerData: { module: wasm, shared: message.shared },
    });
    this.worker.on('message', (data) => this.emit('message', { data }));
    this.worker.on('error', (error) => this.emit('error', error));
  }
  emit(type: string, event: unknown) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
  terminate() {
    void this.worker?.terminate();
  }
}

before(async () => {
  wasm = await WebAssembly.compile(await readFile(join(root, 'target/wasm32-wasip1/release/pagura-host.wasm')));
  manifest = JSON.parse(await readFile(join(root, 'corpus/manifests/classic.json'), 'utf8'));
  scratch = await mkdtemp(join(tmpdir(), 'pagura-host-'));
});
after(async () => {
  client?.destroy();
  await Promise.all(files.map((file) => file.close()));
  if (scratch) await rm(scratch, { recursive: true, force: true });
});

test('web/test_server.py: isolation headers, MIME types and path containment', () => {
  const serverChecks = spawnSync(python, [join(root, 'web/test_server.py')], { encoding: 'utf8', timeout: 30000 });
  assert.ifError(serverChecks.error);
  assert.equal(serverChecks.status, 0, serverChecks.stderr);
});

function bytesEqual(actual: Uint8Array, expected: ArrayLike<number>, message: string) {
  assert.equal(actual.length, expected.length, `${message}: byte count`);
  const first = actual.findIndex((byte, index) => byte !== expected[index]);
  assert.equal(first, -1, `${message}: first different byte ${first}`);
}
function parse(bytes: Uint8Array) {
  const frames: AnyFrame[] = [],
    decoder = new FrameDecoder((frame) => frames.push(frame));
  // Vary chunk boundaries through every fixed-size field and large tile.
  for (let offset = 0; offset < bytes.length; offset += 137) decoder.feed(bytes.subarray(offset, offset + 137));
  decoder.finish();
  return frames;
}
function ppmPixels(bytes: Uint8Array, width: number, height: number) {
  // Fixture generator emits canonical P6, independently of the engine.
  const header = Buffer.from(`P6\n${width} ${height}\n255\n`);
  bytesEqual(bytes.subarray(0, header.length), header, 'oracle PPM header');
  assert.equal(bytes.length - header.length, width * height * 3);
  return bytes.subarray(header.length);
}
// `tolerance` is the manifest's per-channel allowance (0 for exact oracles).
function checkPixels(tile: { rgba: ArrayLike<number> }, rgb: ArrayLike<number>, tolerance = 0) {
  assert.equal(tile.rgba.length, (rgb.length / 3) * 4);
  for (let pixel = 0; pixel < rgb.length / 3; pixel++) {
    for (let channel = 0; channel < 3; channel++) {
      if (Math.abs(tile.rgba[pixel * 4 + channel] - rgb[pixel * 3 + channel]) > tolerance)
        assert.fail(`Oracle mismatch at pixel ${pixel}, channel ${channel}`);
    }
    assert.equal(tile.rgba[pixel * 4 + 3], 255);
  }
}

// host-multipage-navigation: independently stated PDF geometry, not engine pixels.
// Page 2's blue square is [32, 32, 64, 64] in bottom-left PDF coordinates.
// On a 64x64 top-left surface that covers x=[32,64), y=[0,32).
function secondPageRgb(tile: TileRect) {
  const rgb = new Uint8Array(tile.width * tile.height * 3).fill(255);
  for (let y = 0; y < tile.height; y++)
    for (let x = 0; x < tile.width; x++) {
      if (tile.x + x >= 32 && tile.x + x < 64 && tile.y + y >= 0 && tile.y + y < 32) {
        const offset = (y * tile.width + x) * 3;
        rgb[offset] = 0;
        rgb[offset + 1] = 0;
        rgb[offset + 2] = 255;
      }
    }
  return rgb;
}

async function testTwoPages(client: SessionClient, root: string, native: string, wasiRun: WasiRun) {
  const pdf = new Uint8Array(await readFile(join(root, 'corpus/generated/classic/two-pages.pdf')));
  const tiles = [
    { x: 0, y: 0, width: 64, height: 64 },
    { x: 24, y: 16, width: 32, height: 32 },
    { x: 32, y: 0, width: 32, height: 32 },
    { x: 0, y: 32, width: 32, height: 32 },
  ];
  const input = Buffer.concat([
    encodeRequest(1, 1),
    encodeRequest(2, 2, { bytes: pdf, recovery: 0, password: new Uint8Array() }),
    encodeRequest(3, 3, { document: 1, page: 0 }),
    encodeRequest(3, 4, { document: 1, page: 1 }),
    ...tiles.map((tile, index) => encodeRequest(4, index + 5, { document: 1, page: 1, scale: 1, ...tile })),
    encodeRequest(6, 9, { document: 1 }),
  ]);
  const nativeRun = spawnSync(native, [], { input, timeout: 30000 });
  assert.ifError(nativeRun.error);
  assert.equal(nativeRun.status, 0);
  const wasi = await wasiRun(input, 'two-pages');
  assert.equal(wasi.code, 0);
  bytesEqual(wasi.bytes, nativeRun.stdout, 'two-page native/WASI parity');
  const frames = parse(wasi.bytes);
  assert.deepEqual(
    frames.map(({ kind }) => kind),
    [129, 130, 131, 131, 132, 132, 132, 132, 134],
  );
  assert.equal(frames[1].value.pages, 2);
  const expectedInfo = { crop: [0, 0, 64, 64], rotation: 0, userUnit: 1, width: 64, height: 64 };
  assert.deepEqual(frames[2].value, expectedInfo);
  assert.deepEqual(frames[3].value, expectedInfo);
  const opened = await client.open(pdf, 0);
  assert.equal(opened.pages, 2);
  for (const page of [0, 1]) assert.deepEqual(await client.pageInfo(opened.document, page), expectedInfo);
  for (const [index, tile] of tiles.entries()) {
    const expected = secondPageRgb(tile);
    checkPixels(frames[index + 4].value, expected);
    checkPixels(await client.renderTile(opened.document, 1, 1, tile), expected);
  }
  await client.close(opened.document);
}

async function testGenerations(client: SessionClient, root: string, native: string, wasiRun: WasiRun) {
  const pdf = new Uint8Array(await readFile(join(root, 'corpus/generated/classic/rectangle.pdf')));
  const tile = { x: 0, y: 0, width: 64, height: 64 };
  const input = Buffer.concat([
    encodeRequest(1, 1, { version: VERSION_1_1 }),
    encodeRequest(2, 2, { bytes: pdf, recovery: 0, password: new Uint8Array() }),
    encodeRequest(7, 3, { document: 1, generation: 2 }),
    encodeRequest(8, 4, { document: 1, page: 0xffffffff, scale: 1, ...tile, generation: 1 }),
    encodeRequest(8, 5, { document: 1, page: 0, scale: 1, ...tile, generation: 2 }),
  ]);
  const result = spawnSync(native, [], { input, timeout: 30000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0);
  const wasi = await wasiRun(input, 'generations');
  assert.equal(wasi.code, 0);
  bytesEqual(wasi.bytes, result.stdout, 'generation native/WASI parity');
  const frames = parse(wasi.bytes);
  assert.equal(frames[3].kind, 136);
  assert.equal(frames[3].value.cancelled, true);
  const expected = ppmPixels(await readFile(join(root, 'corpus/generated/classic/rectangle.ppm')), 64, 64);
  checkPixels(frames[4].value, expected);
  const opened = await client.open(pdf);
  const old = client.invalidate(opened.document);
  const queued = client.renderGenerationTile(opened.document, 0, 1, tile, old);
  const latest = client.invalidate(opened.document);
  assert.equal((await queued).cancelled, true);
  checkPixels((await client.renderGenerationTile(opened.document, 0, 1, tile, latest)) as Tile, expected);
  await client.close(opened.document);
}

// The parity tests share one session and run in order; the last one ends it.
describe('native/WASI parity and the browser worker shim', () => {
  before(async () => {
    client = new SessionClient(new TestWorker());
    await client.hello();
  });

  test('classic render cases: response identity, PPM oracles and partial tile rows', async (t) => {
    let tested = 0;
    for (const entry of manifest.cases.filter((entry) => entry.expected === 'render')) {
      const pdf = new Uint8Array(await readFile(join(root, entry.path)));
      const tile = { x: 0, y: 0, width: entry.width, height: entry.height };
      const input = Buffer.concat([
        encodeRequest(1, 1),
        encodeRequest(2, 2, { bytes: pdf, password: new Uint8Array(), recovery: 1 }),
        encodeRequest(3, 3, { document: 1, page: 0 }),
        encodeRequest(4, 4, { document: 1, page: 0, scale: 1, ...tile }),
        encodeRequest(5, 5, { document: 1 }),
        encodeRequest(6, 6, { document: 1 }),
      ]);
      const nativeRun = spawnSync(native, [], { input, maxBuffer: 128 * 1024 * 1024, timeout: 30000 });
      assert.ifError(nativeRun.error);
      assert.equal(nativeRun.status, 0, nativeRun.stderr?.toString());
      const wasi = await wasiRun(input, `case-${tested}`);
      assert.equal(wasi.code, 0, wasi.stderr.toString());
      bytesEqual(wasi.bytes, nativeRun.stdout, `${entry.id}: native/WASI response identity`);
      const frames = parse(wasi.bytes);
      assert.deepEqual(
        frames.map((frame) => frame.kind),
        [129, 130, 131, 132, 133, 134],
      );
      assert.deepEqual(
        frames.map((frame) => frame.id),
        [1, 2, 3, 4, 5, 6],
      );
      const rgb = ppmPixels(await readFile(join(root, entry.oracle)), entry.width, entry.height);
      checkPixels(frames[3].value, rgb, entry.tolerance ?? 0);
      const opened = await client.open(pdf);
      const info = await client.pageInfo(opened.document, 0);
      assert.equal(info.width, entry.width);
      assert.equal(info.height, entry.height);
      const browserTile = await client.renderTile(opened.document, 0, 1, tile);
      bytesEqual(browserTile.rgba, frames[3].value.rgba, `${entry.id}: browser shim/native pixels`);
      const diagnostics = await client.diagnostics(opened.document);
      assert.deepEqual(diagnostics, frames[4].value);
      assert.deepEqual(
        diagnostics.map((item) => item.tag),
        entry.repairs,
      );
      // Partial device rectangles exercise the tile offset and edge dimensions.
      for (const partial of [
        { x: 0, y: 0, width: Math.min(31, entry.width), height: entry.height },
        { x: entry.width - 1, y: entry.height - 1, width: 1, height: 1 },
      ]) {
        const rendered = await client.renderTile(opened.document, 0, 1, partial);
        for (let y = 0; y < partial.height; y++) {
          const expectedStart = ((partial.y + y) * entry.width + partial.x) * 4;
          bytesEqual(
            rendered.rgba.subarray(y * partial.width * 4, (y + 1) * partial.width * 4),
            browserTile.rgba.subarray(expectedStart, expectedStart + partial.width * 4),
            `${entry.id}: partial tile row`,
          );
        }
      }
      await client.close(opened.document);
      await assert.rejects(
        client.pageInfo(opened.document, 0),
        (error) => error instanceof HostError && error.category === 7,
      );
      tested++;
    }
    assert.ok(tested > 0, 'The classic manifest must contain render cases');
    t.diagnostic(`${tested} classic render cases`);
  });
  test('reading, search and navigation (testReading)', () => testReading(client, root, native, wasiRun));
  test('multi-page navigation (testTwoPages)', () => testTwoPages(client, root, native, wasiRun));
  test('viewport generations (testGenerations)', () => testGenerations(client, root, native, wasiRun));
  test('retention stats and native/WASI parity', async () => {
    const emptyStats = { sourceBytes: 0, cacheBytes: 0, documents: 0, lists: 0 };
    assert.deepEqual(await client.stats(), emptyStats);
    const retainedPdf = new Uint8Array(await readFile(join(root, 'corpus/generated/classic/rectangle.pdf')));
    const retained = await client.open(retainedPdf);
    await client.renderTile(retained.document, 0, 1, { x: 0, y: 0, width: 64, height: 64 });
    assert.deepEqual(await client.stats(), {
      sourceBytes: retainedPdf.length,
      cacheBytes: 4608,
      documents: 1,
      lists: 1,
    });
    assert.ok(client.wasmBytes > 0);
    await client.close(retained.document);
    assert.deepEqual(await client.stats(), emptyStats);
    const statsInput = Buffer.concat([encodeRequest(1, 1, { version: VERSION_1_1 }), encodeRequest(9, 2)]);
    const statsNative = spawnSync(native, [], { input: statsInput, timeout: 30000 });
    assert.equal(statsNative.status, 0);
    bytesEqual((await wasiRun(statsInput, 'stats')).bytes, statsNative.stdout, 'retention native/WASI parity');
    assert.deepEqual(parse(statsNative.stdout)[1].value, emptyStats);
  });
  test('client deadlines and disabled failure injection', async () => {
    assert.throws(() => client.injectFailure('trap'), /disabled/);
    const hung = new SessionClient({ addEventListener() {}, postMessage() {}, terminate() {} }, { timeoutMs: 10 });
    await assert.rejects(hung.hello(), (error) => error instanceof HostFailure && error.reason === 'deadline exceeded');
    const deadlineListeners = new Map();
    const stalled = new SessionClient(
      { addEventListener: (type, fn) => deadlineListeners.set(type, fn), postMessage() {}, terminate() {} },
      { timeoutMs: 10 },
    );
    deadlineListeners.get('message')({ data: { type: 'ready' } });
    await assert.rejects(
      stalled.pageInfo(1, 0),
      (error) => error instanceof HostFailure && error.reason === 'deadline exceeded',
    );
  });
  test('structured Unsupported and strict-repair failures', async () => {
    // Explicit unsupported failures survive browser framing with context/tag.
    const unsupportedCase = manifest.cases.find((entry) => entry.expected === 'unsupported');
    assert.ok(unsupportedCase);
    const unsupported = await client.open(new Uint8Array(await readFile(join(root, unsupportedCase.path))));
    const unsupportedInfo = await client.pageInfo(unsupported.document, 0);
    await assert.rejects(
      client.renderTile(unsupported.document, 0, 1, {
        x: 0,
        y: 0,
        width: unsupportedInfo.width,
        height: unsupportedInfo.height,
      }),
      (error) => error instanceof HostError && error.category === 3 && error.tag === unsupportedCase.feature,
    );
    await client.close(unsupported.document);
    const trailing = new Uint8Array(await readFile(join(root, 'corpus/generated/classic/trailing-bytes.pdf')));
    await assert.rejects(
      client.open(trailing, 0),
      (error) =>
        error instanceof HostError &&
        error.category === 1 &&
        error.tag === '' &&
        error.repair === 'file.trailing-bytes',
    );
    const strictInput = Buffer.concat([
      encodeRequest(1, 1),
      encodeRequest(2, 2, { bytes: trailing, password: new Uint8Array(), recovery: 0 }),
    ]);
    const strictNative = spawnSync(native, [], { input: strictInput, timeout: 30000 });
    assert.ifError(strictNative.error);
    assert.equal(strictNative.status, 0);
    const strictWasi = await wasiRun(strictInput, 'strict-repair');
    assert.equal(strictWasi.code, 0);
    bytesEqual(strictWasi.bytes, strictNative.stdout, 'strict repair native/WASI parity');
    assert.equal(parse(strictWasi.bytes)[1].value.repair, 'file.trailing-bytes');
  });
  test('malformed frames are terminal on both targets', async (t) => {
    const malformed = [
      Buffer.from([255, 255, 255, 255]),
      Buffer.from([1, 0]),
      Buffer.from([0, 0, 0, 0]),
      Buffer.from([5, 0, 0, 0, 77, 2, 0, 0, 0]),
      encodeRequest(5, 1, { document: 1 }),
      Buffer.concat([encodeRequest(1, 1), encodeRequest(1, 2)]),
    ];
    for (let index = 0; index < malformed.length; index++) {
      const nativeRun = spawnSync(native, [], { input: malformed[index], timeout: 30000 });
      assert.ifError(nativeRun.error);
      assert.equal(nativeRun.status, 1);
      const wasi = await wasiRun(malformed[index], `bad-${index}`);
      assert.equal(wasi.code, 1);
      bytesEqual(wasi.bytes, nativeRun.stdout, 'malformed native/WASI parity');
      assert.equal(parse(wasi.bytes).at(-1)!.kind, 255);
    }
    t.diagnostic(`${malformed.length} terminal failures`);
  });
  test('the worker exits cleanly at stdin EOF', async () => {
    const cleanExit = new Promise((done, reject) => {
      const timer = setTimeout(() => reject(new Error('Worker failed to exit at stdin EOF')), 10000);
      client.worker.addEventListener('message', ({ data }) => {
        if (data.type === 'exit') {
          clearTimeout(timer);
          done(data.code);
        }
      });
    });
    client.input.end();
    assert.equal(await cleanExit, 0);
  });
});
test('client failures (testClientFailures)', testClientFailures);
test('codec and capabilities (testCodecAndCapabilities)', testCodecAndCapabilities);
test('fixture loading (testFixtureLoading)', testFixtureLoading);
describe('viewer DOM harness (testViewer)', () => {
  let startupPdf: Buffer;
  before(async () => {
    startupPdf = await readFile(join(root, 'corpus/generated/classic/two-pages.pdf'));
  });
  test('continuous layout, navigation, recovery, reading and link policy', () =>
    testViewer(TestWorker, root, manifest));
  const startups: Startup[] = [
    {
      name: 'fixture',
      url: 'http://localhost:8000/?file=/fixtures/classic/two-pages.pdf',
      fetch: async () => new Response(startupPdf as Uint8Array<ArrayBuffer>),
    },
    {
      name: 'disabled',
      url: 'http://localhost:8000/?file=/fixtures/classic/two-pages.pdf',
      fetch: async () => new Response('', { status: 404 }),
      error: /HTTP 404.*--fixtures/,
    },
    {
      name: 'external',
      url: 'http://localhost:8000/?file=https://example.invalid/fixtures/file.pdf',
      fetch: async () => assert.fail('External URL reached fetch'),
      error: /same-origin/,
    },
    {
      name: 'private',
      url: 'http://localhost:8000/?file=/Cargo.toml',
      fetch: async () => assert.fail('Private path reached fetch'),
      error: /same-origin/,
    },
    {
      name: 'duplicate',
      url: 'http://localhost:8000/?file=/fixtures/a.pdf&file=/fixtures/b.pdf',
      fetch: async () => assert.fail('Ambiguous startup URL reached fetch'),
      error: /specified once/,
    },
  ];
  for (const startup of startups)
    test(`startup: ${startup.name}`, () => testViewer(TestWorker, root, manifest, startup));
});
// Opt-in builds use the same font feature on native and WASI. The default
// module above remains the reference for data-free parity.
describe('bundled-data host: protocol 1.4 diagnostic kinds', () => {
  before(async () => {
    wasm = await WebAssembly.compile(
      await readFile(join(root, 'target/host-bundled/wasm32-wasip1/release/pagura-host.wasm')),
    );
  });
  test('diagnostic kinds (testDiagnosticKinds)', async () => {
    const bundled = new SessionClient(new TestWorker());
    try {
      await bundled.hello();
      await testDiagnosticKinds(bundled, root, join(root, 'target/host-bundled/release/pagura-host'), wasiRun);
    } finally {
      bundled.destroy();
    }
  });
  test('viewer mixed diagnostic counts', () => testViewer(TestWorker, root, manifest, null, true));
});

async function testClientFailures() {
  assert.equal(failureReason(new WebAssembly.RuntimeError('unreachable')), 'trap');
  assert.equal(failureReason(new RangeError('out of memory')), 'memory failure');
  assert.equal(failureReason(new Error('memory allocation failed')), 'memory failure');
  for (const reason of ['trap', 'memory failure', 'budget exceeded']) {
    const listeners = new Map();
    let terminated = 0,
      notified = 0;
    const thread = {
      addEventListener: (type: string, fn: Listener) => listeners.set(type, fn),
      postMessage() {},
      terminate: () => {
        terminated++;
      },
    };
    const client = new SessionClient(thread, {
      onFailure: () => {
        notified++;
      },
    });
    const message = (data: object) => listeners.get('message')({ data });
    message({ type: 'ready', wasmBytes: 65536 });
    const first = client.pageInfo(1, 0),
      queued = client.stats();
    const rejected = [first, queued].map((promise) =>
      assert.rejects(promise, (error) => error instanceof HostFailure && error.reason === reason),
    );
    await new Promise((done) => setTimeout(done, 0));
    if (reason === 'budget exceeded') {
      client.receive({ kind: 255, id: 1, value: new HostError(2, '', '', 'tile response budget exceeded', {}) });
    } else message({ type: 'fatal', reason, message: 'execution failed' });
    await Promise.all(rejected);
    assert.equal(terminated, 1);
    assert.equal(notified, 1);
    message({ type: 'stdout', bytes: new Uint8Array([255]) });
    client.destroy();
    assert.equal(terminated, 1);
    assert.equal(notified, 1);
    assert.equal(client.decoder.body, null);
  }
}

function testCodecAndCapabilities() {
  const layout = new PageLayout(3);
  assert.equal(layout.top(2, 1), 1632);
  layout.setHeight(0, 64);
  layout.setHeight(1, 128);
  assert.equal(layout.top(2, 2), 432);
  assert.equal(layout.pageAt(431, 2), 1);
  assert.equal(layout.pageAt(432, 2), 2);
  assert.throws(() => layout.setHeight(0, NaN));
  const pages = new PageLayout(3);
  for (let i = 0; i < 3; i++) pages.setHeight(i, 100);
  assert.equal(pages.dominantPage(120, 100, 1, 0), 1); // Eight pixels of the preceding page remain at the top.
  assert.equal(pages.dominantPage(0, 400, 1, 2), 2); // Equal coverage retains an explicit destination.
  assert.equal(pages.dominantPage(120, 100, 1, 2), 1);
  assert.equal(pages.dominantPage(244, 100, 2, 0), 1);
  for (const flag of [2, 255])
    assert.throws(() => decodeResponse(Uint8Array.from([144, 1, 0, 0, 0, flag])), /bounds flag/);
  const literalBounds = Buffer.concat([
    Buffer.from([144, 1, 0, 0, 0, 1]),
    ...[60, 100, 180, 300].map((v) => {
      const b = Buffer.alloc(8);
      b.writeDoubleLE(v);
      return b;
    }),
  ]);
  assert.deepEqual(decodeResponse(literalBounds).value, { bounds: [60, 100, 180, 300] });
  for (let n = 6; n < literalBounds.length; n++)
    assert.throws(() => decodeResponse(literalBounds.subarray(0, n)), /Truncated/);
  const invalidBounds = Buffer.from(literalBounds);
  invalidBounds.writeDoubleLE(NaN, 6);
  assert.throws(() => decodeResponse(invalidBounds), /Invalid content bounds/);
  invalidBounds.writeDoubleLE(999, 6);
  assert.throws(() => decodeResponse(invalidBounds), /Invalid content bounds/);
  assert.throws(() => decodeResponse(Buffer.concat([literalBounds, Buffer.from([0])])), /Trailing/);
  bytesEqual(
    encodeRequest(1, 7),
    Uint8Array.from([9, 0, 0, 0, 1, 7, 0, 0, 0, 1, 0, 0, 0]),
    'literal JS handshake oracle',
  );
  for (const bytes of [new Uint8Array([0, 0, 0, 0]), new Uint8Array([255, 255, 255, 255])]) {
    assert.throws(() => new FrameDecoder(() => {}).feed(bytes));
  }
  const decoder = new FrameDecoder(() => {});
  decoder.feed(new Uint8Array([1]));
  assert.throws(() => decoder.finish());
  assert.throws(() => encodeRequest(4, 1, { document: 1, page: 0, scale: NaN }));
  const refused = Buffer.concat([
    Buffer.from([255, 2, 0, 0, 0, 1, 0, 0, 0, 0, 19, 0, 0, 0]),
    Buffer.from('file.trailing-bytes'),
    Buffer.from([3, 0, 0, 0]),
    Buffer.from('bad'),
    Buffer.from([0, 0]),
  ]);
  const failure = decodeResponse(refused).value as HostError;
  assert.equal(failure.category, 1);
  assert.equal(failure.tag, '');
  assert.equal(failure.repair, 'file.trailing-bytes');
  assert.match(String(failure), /recovery would repair it \[file\.trailing-bytes\]/);
  for (const category of [2, 5]) {
    const originalCategory = refused.slice();
    originalCategory[5] = category;
    const preserved = decodeResponse(originalCategory).value as HostError;
    assert.equal(preserved.category, category);
    assert.equal(preserved.repair, 'file.trailing-bytes');
  }
  const wrongCategory = refused.slice();
  wrongCategory[5] = 3;
  assert.throws(() => decodeResponse(wrongCategory), /Invalid structured error/);
  const draft = Buffer.from([255, 2, 0, 0, 0, 1, 0, 0, 0, 0, 3, 0, 0, 0, 98, 97, 100, 0, 0]);
  assert.throws(() => decodeResponse(draft), /Truncated protocol response/);
  const memory = new WebAssembly.Memory({ initial: 1 });
  const wasi = createWasi(
    () => memory,
    (bytes) => {
      bytes.fill(7);
      return bytes.length;
    },
    () => {},
  );
  const view = new DataView(memory.buffer);
  view.setUint32(0, 64, true);
  view.setUint32(4, 16, true);
  assert.equal(wasi.fd_read(0, 0, 1, 32), 0);
  assert.equal(view.getUint32(32, true), 16);
  assert.equal(wasi.fd_read(3, 0, 1, 32), 8);
  assert.equal(wasi.fd_write(0, 0, 1, 32), 8);
  for (const descriptor of [0, 1, 2, 3, 99]) assert.equal(wasi.fd_prestat_get(descriptor, 0), 8);
  assert.equal(wasi.args_sizes_get(32, 36), 0);
  assert.equal(view.getUint32(32, true), 0);
  assert.equal(view.getUint32(36, true), 0);
  assert.equal(wasi.environ_sizes_get(32, 36), 0);
  assert.equal(view.getUint32(32, true), 0);
  assert.equal(view.getUint32(36, true), 0);
  assert.equal(wasi.clock_time_get(99, 0n, 32), 28);
  assert.equal(wasi.clock_time_get(1, 0n, 32), 0);
  assert.throws(() => wasi.fd_write(1, 65535, 1, 32), /outside linear memory/);
  assert.throws(() => wasi.path_open(), /Unsupported WASI import: wasi_snapshot_preview1.path_open/);
  assert.throws(() => wasi.sock_open(), /Unsupported WASI import/);
  assert.throws(
    () => wasi.proc_exit(7),
    (error) => (error as { code?: number }).code === 7,
  );
  memory.grow(1); // All shim views must be rebuilt after growth detaches old views.
  assert.equal(wasi.random_get(65536, 65536), 0);
  assert.equal(wasi.clock_time_get(0, 0n, 65536), 0);
  assert.throws(() => wasi.random_get(-1, 8), /outside linear memory/);
}

/** Exercise the actual viewer module without claiming browser layout coverage.
 * Minimal DOM/Canvas stand-ins expose event and state errors, while all PDF
 * pixels still come from the real WASM command in a worker and the actual shim.
 */
async function testViewer(
  TestWorker: new () => object,
  root: string,
  manifest: Manifest,
  startup: Startup | null = null,
  diagnosticKinds = false,
) {
  /** Image data captured by the canvas stand-in's putImageData. */
  interface Painted {
    data: Uint8ClampedArray;
    width: number;
    height: number;
  }
  // One stand-in for elements and the window; `declare` fields add no properties.
  class Element {
    declare id: string;
    declare tagName: string;
    declare children: Element[];
    declare listeners: Map<string, Listener[]>;
    declare style: Record<string, string>;
    declare attributes: Map<string, string>;
    declare hidden: boolean;
    declare value: string;
    declare scrollTop: number;
    declare scrollLeft: number;
    declare clientWidth: number;
    declare clientHeight: number;
    declare checked: boolean;
    declare textContent: string;
    declare parent?: Element | null;
    declare className?: string;
    declare disabled?: boolean;
    declare files?: File[];
    declare width?: number;
    declare height?: number;
    declare image?: Painted;
    /** A dialog's state, or window.open on the window stand-in. */
    declare open?: boolean | ((url: string, target: string, features: string) => void);
    declare devicePixelRatio?: number;
    declare location?: URL;
    declare getSelection?: () => unknown;
    declare confirm?: (message: string) => boolean;
    declare paguraDebug?: { fail(reason: string): void };
    constructor(id: string) {
      this.id = id;
      this.tagName = id.toUpperCase();
      this.children = [];
      this.listeners = new Map();
      this.style = {};
      this.attributes = new Map();
      this.hidden = ['page-surface', 'error', 'find-panel'].includes(id);
      this.value = '1';
      this.scrollTop = 0;
      this.scrollLeft = 0;
      this.clientWidth = 300;
      this.clientHeight = 300;
      this.checked = false;
      this.textContent = '';
      if (id === 'find-query') this.value = '';
      if (['page', 'find-query'].includes(id)) this.tagName = 'INPUT';
    }
    addEventListener(type: string, listener: Listener) {
      const list = this.listeners.get(type) ?? [];
      list.push(listener);
      this.listeners.set(type, list);
    }
    removeEventListener(type: string, listener: Listener) {
      this.listeners.set(
        type,
        (this.listeners.get(type) ?? []).filter((fn) => fn !== listener),
      );
    }
    dispatch(type: string, extra: object = {}) {
      for (const fn of [...(this.listeners.get(type) ?? [])]) fn({ target: this, preventDefault() {}, ...extra });
    }
    replaceChildren(...children: Element[]) {
      for (const child of this.children) child.parent = null;
      this.children = children;
      for (const child of children) child.parent = this;
    }
    append(child: Element) {
      this.children.push(child);
      child.parent = this;
    }
    insertBefore(child: Element, next: Element | null) {
      const index = next ? this.children.indexOf(next) : this.children.length;
      this.children.splice(index, 0, child);
      child.parent = this;
    }
    remove() {
      if (this.parent) this.parent.children = this.parent.children.filter((child) => child !== this);
    }
    setAttribute(name: string, value: string) {
      this.attributes.set(name, value);
    }
    getAttribute(name: string) {
      return this.attributes.get(name);
    }
    removeAttribute(name: string) {
      this.attributes.delete(name);
    }
    focus() {
      (globalThis.document as unknown as { activeElement: Element | null }).activeElement = this;
    }
    click() {
      this.dispatch('click');
    }
    closest(selector: string): Element | undefined {
      if (selector === '.text-layer' && this.className === 'text-layer') return this;
      if (selector === '[role=tree]' && this.getAttribute('role') === 'tree') return this;
      return this.parent?.closest(selector);
    }
    showModal() {
      this.open = true;
    }
    close() {
      this.open = false;
    }
    getBoundingClientRect() {
      const viewport = elements.get('viewport');
      if (this.id === 'viewport')
        return { left: 0, top: 0, right: viewport.clientWidth, bottom: viewport.clientHeight };
      const width = parseFloat(this.style.width) || 0,
        height = parseFloat(this.style.height) || 0;
      const left =
        Math.max(0, ((parseFloat(elements.get('page-surface').style.width) || width) - width) / 2) -
        viewport.scrollLeft;
      const top = (parseFloat(this.style.top) || 0) - viewport.scrollTop;
      return { left, top, right: left + width, bottom: top + height };
    }
    getContext(kind: string) {
      assert.equal(kind, '2d');
      return {
        putImageData: (image: Painted) => {
          this.image = image;
        },
        measureText: () => ({ width: 0.6 }),
      };
    }
  }
  const ids = [
    'page-surface',
    'viewport',
    'status',
    'error',
    'page',
    'zoom',
    'previous',
    'next',
    'count',
    'file',
    'strict',
    'filename',
    'diagnostic-count',
    'repair-list',
    'diagnostics',
    'custom-zoom',
    'password-dialog',
    'password',
    'password-feedback',
    'password-form',
    'password-cancel',
    'find-open',
    'find-close',
    'find-panel',
    'find-query',
    'find-case',
    'find-word',
    'find-next',
    'find-previous',
    'find-count',
    'outline-tree',
    'page-label',
    'reading-status',
    'copy-page',
  ];
  // Every id above exists, so lookups by those ids always find an element.
  const elements = new Map(ids.map((id): [string, Element] => [id, new Element(id)])) as {
    get(id: string): Element;
  } & Map<string, Element>;
  const active = () => (globalThis.document as unknown as { activeElement: Element }).activeElement;
  const original = new Map(
    [
      'window',
      'document',
      'Worker',
      'ImageData',
      'requestAnimationFrame',
      'crossOriginIsolated',
      'fetch',
      'navigator',
      'ResizeObserver',
    ].map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]),
  );
  const fakeWindow = new Element('window');
  fakeWindow.devicePixelRatio = 1;
  let observedResize!: () => void,
    disconnected = false;
  let copied = '';
  fakeWindow.getSelection = () => null;
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: {
      clipboard: {
        writeText: async (text: string) => {
          copied = text;
        },
      },
    },
  });
  fakeWindow.location = new URL(startup?.url ?? 'http://localhost:8000/?debug=1');
  Object.assign(globalThis, {
    window: fakeWindow,
    document: {
      activeElement: null,
      addEventListener: (...args: [string, Listener]) => fakeWindow.addEventListener(...args),
      getElementById: (id: string) => elements.get(id),
      createElement: (name: string) => new Element(name),
      body: { classList: { add() {}, remove() {} } },
    },
    Worker: TestWorker,
    crossOriginIsolated: true,
    ResizeObserver: class {
      constructor(fn: () => void) {
        observedResize = fn;
      }
      observe(node: Element) {
        assert.equal(node, elements.get('viewport'));
      }
      disconnect() {
        disconnected = true;
      }
    },
    requestAnimationFrame: (fn: () => void) => setTimeout(fn, 0),
    ImageData: class {
      declare data: Uint8ClampedArray;
      declare width: number;
      declare height: number;
      constructor(data: Uint8ClampedArray, width: number, height: number) {
        this.data = data;
        this.width = width;
        this.height = height;
      }
    },
  });
  if (startup) globalThis.fetch = startup.fetch;
  const waitFor = async (predicate: () => unknown) => {
    const deadline = performance.now() + 10000;
    while (!predicate()) {
      if (performance.now() >= deadline)
        assert.fail(
          `Viewer did not settle: ${elements.get('status').textContent}; ${elements.get('error').textContent}`,
        );
      await new Promise((done) => setTimeout(done, 5));
    }
  };
  const pick = async (entry: { id: string; path: string }) => {
    const picker = elements.get('file');
    picker.files = [new File([await readFile(join(root, entry.path))], entry.id + '.pdf')];
    picker.dispatch('change');
  };
  const ready = () => elements.get('status').textContent === 'Visible tiles rendered';
  const view = (index: number) =>
    elements.get('page-surface').children.find((element) => element.getAttribute('data-page') === String(index));
  const canvases = (index = 0) => view(index)?.children.filter((element) => element.tagName === 'CANVAS') ?? [];
  try {
    await import(`../web/viewer.mjs?test=${diagnosticKinds ? 'diagnostic-kinds' : (startup?.name ?? 'continuous')}`);
    if (startup) {
      assert.equal(fakeWindow.paguraDebug, undefined);
      if (startup.error) {
        assert.equal(elements.get('error').hidden, false);
        assert.match(elements.get('error').textContent, startup.error);
        assert.equal(elements.get('page-surface').children.length, 0);
      } else {
        await waitFor(ready);
        assert.equal(elements.get('filename').textContent, 'two-pages.pdf');
        assert.equal(elements.get('count').textContent, 'of 2');
        const first = manifest.cases.find((entry) => entry.id === 'classic/two-pages')!;
        checkPixels({ rgba: canvases()[0].image!.data }, ppmPixels(await readFile(join(root, first.oracle)), 64, 64));
        assert.equal(view(0)!.getAttribute('role'), 'region');
      }
      return;
    }
    assert.equal(elements.get('status').textContent, 'Ready — open a PDF');
    if (diagnosticKinds) {
      const mixed = {
        id: 'paint/bundled-diagnostic-kinds',
        path: 'corpus/generated/paint/bundled-diagnostic-kinds.pdf',
      };
      await pick(mixed);
      await waitFor(ready);
      assert.equal(elements.get('diagnostic-count').textContent, 'Repairs: 1 · Information: 1');
      assert.deepEqual(
        elements.get('repair-list').children.map((item) => item.textContent.split(' [')[0]),
        ['Repair', 'Information'],
      );
      return;
    }
    for (const entry of manifest.cases.filter((entry) => entry.expected === 'render')) {
      await pick(entry);
      await waitFor(ready);
      assert.equal(elements.get('error').hidden, true);
      assert.equal(
        elements.get('diagnostic-count').textContent,
        `Repairs: ${entry.repairs.length} · Information: ${(entry.information ?? []).length}`,
      );
      assert.equal(elements.get('page').value, '1');
      const rgb = ppmPixels(await readFile(join(root, entry.oracle)), entry.width, entry.height);
      assert.ok(canvases().length > 0);
      for (const canvas of canvases()) {
        const x = parseFloat(canvas.style.left),
          y = parseFloat(canvas.style.top),
          rgba = canvas.image!.data;
        for (let row = 0; row < canvas.height!; row++)
          for (let column = 0; column < canvas.width!; column++) {
            const pixel = (row * canvas.width! + column) * 4,
              oracle = ((y + row) * entry.width + x + column) * 3;
            const differs = [0, 1, 2].some(
              (channel) => Math.abs(rgba[pixel + channel] - rgb[oracle + channel]) > (entry.tolerance ?? 0),
            );
            if (differs || rgba[pixel + 3] !== 255)
              assert.fail(`Viewer canvas mismatch: ${entry.id}, (${x + column}, ${y + row})`);
          }
      }
      if (entry.id === 'classic/many-rectangles') {
        const viewport = elements.get('viewport');
        viewport.scrollTop = 400;
        viewport.dispatch('scroll');
        await waitFor(
          () =>
            canvases().length === 2 && canvases().every((canvas) => parseFloat(canvas.style.top) === 256) && ready(),
        );
        viewport.scrollTop = 0;
        viewport.dispatch('scroll');
        await waitFor(() => canvases().length === 4 && ready());
      }
    }
    const twoPages = manifest.cases.find((entry) => entry.id === 'classic/two-pages')!;
    elements.get('viewport').clientHeight = 40;
    await pick(twoPages);
    await waitFor(ready);
    assert.ok(view(0));
    assert.equal(view(1), undefined);
    elements.get('viewport').scrollTop = 112;
    elements.get('viewport').dispatch('scroll');
    await waitFor(() => ready() && canvases(1).length > 0);
    assert.ok(view(0) === undefined, 'Offscreen page element must be evicted');
    checkPixels({ rgba: canvases(1)[0].image!.data }, secondPageRgb({ x: 0, y: 0, width: 64, height: 64 }));
    elements.get('viewport').clientHeight = 300;
    elements.get('viewport').scrollTop = 0;
    elements.get('viewport').dispatch('scroll');
    await waitFor(() => ready() && canvases(0).length > 0);
    // Both pages coexist when visible, with independently checked content.
    assert.ok(view(0));
    assert.ok(view(1));
    assert.deepEqual(
      elements.get('page-surface').children.map((n) => n.getAttribute('data-page')),
      ['0', '1'],
    );
    const firstPage = ppmPixels(await readFile(join(root, twoPages.oracle)), 64, 64);
    const checkPage = (index: number) => {
      assert.equal(elements.get('page').value, String(index + 1));
      assert.equal(elements.get('count').textContent, 'of 2');
      assert.equal(elements.get('previous').disabled, index === 0);
      assert.equal(elements.get('next').disabled, index === 1);
      checkPixels(
        { rgba: canvases(index)[0].image!.data },
        index === 0 ? firstPage : secondPageRgb({ x: 0, y: 0, width: 64, height: 64 }),
      );
    };
    const settledPage = (index: number) =>
      ready() && elements.get('page').value === String(index + 1) && canvases(index).length > 0;
    // With both pages fully visible, equal coverage retains the last destination.
    assert.equal(elements.get('page').value, '2');
    elements.get('page').value = '1';
    elements.get('page').dispatch('change');
    await waitFor(() => settledPage(0));
    checkPage(0);
    elements.get('next').dispatch('click');
    await waitFor(() => settledPage(1));
    checkPage(1);
    elements.get('previous').dispatch('click');
    await waitFor(() => settledPage(0));
    checkPage(0);
    for (const index of [1, 0, 1]) {
      elements.get('page').value = String(index + 1);
      elements.get('page').dispatch('change');
      await waitFor(() => settledPage(index));
      checkPage(index);
    }
    fakeWindow.dispatch('keydown', { key: 'Home' });
    await waitFor(() => settledPage(0));
    checkPage(0);
    fakeWindow.dispatch('keydown', { key: 'PageDown' });
    await waitFor(() => settledPage(1));
    checkPage(1);
    for (const reason of ['trap', 'memory', 'budget', 'deadline']) {
      await pick(twoPages);
      await waitFor(ready);
      elements.get('zoom').value = '2';
      elements.get('zoom').dispatch('change');
      elements.get('page').value = '2';
      elements.get('page').dispatch('change');
      await waitFor(() => settledPage(1) && view(1)!.style.width === '128px');
      fakeWindow.paguraDebug!.fail(reason);
      await waitFor(() => ready() && elements.get('error').textContent.includes('document reopened'));
      assert.equal(elements.get('page').value, '2');
      assert.equal(view(1)!.style.width, '128px');
      assert.equal(elements.get('error').hidden, false);
      assert.match(elements.get('error').textContent, /page 2, 200%/);
      const expected = [];
      for (let y = 0; y < 128; y++)
        for (let x = 0; x < 128; x++) expected.push(...(x >= 64 && y < 64 ? [0, 0, 255] : [255, 255, 255]));
      checkPixels({ rgba: canvases(1)[0].image!.data }, expected);
      fakeWindow.paguraDebug!.fail(reason);
      await waitFor(() => elements.get('error').textContent.includes('Automatic recovery stopped'));
      assert.equal(canvases(1).length, 0);
      elements.get('zoom').value = '1';
    }
    const rectangle = manifest.cases.find((entry) => entry.id === 'classic/rectangle')!;
    await pick(rectangle);
    await waitFor(ready);
    elements.get('zoom').value = '2';
    elements.get('zoom').dispatch('change');
    elements.get('zoom').value = '1';
    elements.get('zoom').dispatch('change');
    await waitFor(ready);
    assert.equal(view(0)!.style.width, '64px');
    fakeWindow.devicePixelRatio = 2;
    fakeWindow.dispatch('resize');
    await waitFor(() => ready() && canvases()[0]?.width === 128);
    assert.equal(view(0)!.style.width, '64px');
    fakeWindow.devicePixelRatio = 1;
    fakeWindow.dispatch('resize');
    await waitFor(() => ready() && canvases()[0]?.width === 64);
    elements.get('zoom').value = 'fit-width';
    elements.get('zoom').dispatch('change');
    await waitFor(ready);
    assert.equal(view(0)!.style.width, '252px');
    elements.get('viewport').clientHeight = 200;
    elements.get('zoom').value = 'fit-page';
    elements.get('zoom').dispatch('change');
    await waitFor(ready);
    assert.equal(view(0)!.style.height, '152px');
    elements.get('viewport').clientHeight = 180;
    observedResize();
    await waitFor(() => ready() && view(0)!.style.height === '132px');
    elements.get('viewport').clientHeight = 200;
    observedResize();
    await waitFor(() => ready() && view(0)!.style.height === '152px');
    let prevented = false;
    elements.get('viewport').dispatch('wheel', {
      ctrlKey: true,
      deltaY: -50,
      deltaMode: 0,
      clientY: 100,
      preventDefault() {
        prevented = true;
      },
    });
    await waitFor(ready);
    assert.equal(prevented, true);
    assert.ok(parseFloat(view(0)!.style.width) > 152);
    elements.get('viewport').dispatch('touchstart', {
      touches: [
        { clientX: 0, clientY: 0 },
        { clientX: 10, clientY: 0 },
      ],
    });
    const beforePinch = parseFloat(view(0)!.style.width);
    elements.get('viewport').dispatch('touchmove', {
      touches: [
        { clientX: 0, clientY: 0 },
        { clientX: 20, clientY: 0 },
      ],
    });
    await waitFor(ready);
    assert.ok(parseFloat(view(0)!.style.width) > beforePinch);
    elements.get('viewport').dispatch('touchend');
    fakeWindow.dispatch('keydown', { key: '0' });
    await waitFor(ready);
    assert.equal(view(0)!.style.width, '64px');
    elements.get('viewport').clientHeight = 300;
    await pick(manifest.cases.find((entry) => entry.expected === 'unsupported')!);
    await waitFor(() => !elements.get('error').hidden);
    assert.match(elements.get('error').textContent, /Unsupported \[[^\]]+\]/);
    assert.equal(canvases().length, 0);
    fakeWindow.dispatch('drop', {
      dataTransfer: { files: [new File([await readFile(join(root, rectangle.path))], 'dropped.pdf')] },
    });
    await waitFor(ready);
    assert.equal(elements.get('filename').textContent, 'dropped.pdf');
    elements.get('page').value = '2';
    elements.get('page').dispatch('change');
    assert.equal(elements.get('page').value, '1');
    elements.get('strict').checked = true;
    await pick(manifest.cases.find((entry) => entry.id === 'classic/trailing-bytes')!);
    await waitFor(() => !elements.get('error').hidden);
    assert.match(elements.get('error').textContent, /Syntax.*recovery would repair it \[file\.trailing-bytes\]/);
    assert.equal(elements.get('page').disabled, true);
    elements.get('strict').checked = false;
    const crypto: Manifest = JSON.parse(await readFile(join(root, 'corpus/manifests/crypto.json'), 'utf8'));
    const encrypted = crypto.cases.find(
      (entry) => entry.expected === 'render' && entry.password === 'user-secret' && entry.id.includes('r2-'),
    );
    assert.ok(encrypted);
    await pick(encrypted);
    await waitFor(() => elements.get('password-dialog').open);
    elements.get('password').value = 'wrong';
    elements.get('password-form').dispatch('submit');
    await waitFor(
      () => elements.get('password-dialog').open && elements.get('password-feedback').textContent.includes('Incorrect'),
    );
    elements.get('password').value = encrypted.password!;
    elements.get('password-form').dispatch('submit');
    await waitFor(ready);
    assert.equal(elements.get('password').value, '');
    assert.equal(elements.get('password-dialog').open, false);
    checkPixels(
      { rgba: canvases()[0].image!.data },
      ppmPixels(await readFile(join(root, encrypted.oracle)), encrypted.width, encrypted.height),
    );
    fakeWindow.paguraDebug!.fail('trap');
    await waitFor(() => ready() && elements.get('error').textContent.includes('document reopened'));
    assert.equal(elements.get('password-dialog').open, false);
    checkPixels(
      { rgba: canvases()[0].image!.data },
      ppmPixels(await readFile(join(root, encrypted.oracle)), encrypted.width, encrypted.height),
    );
    await pick(encrypted);
    await waitFor(() => elements.get('password-dialog').open);
    elements.get('password-cancel').dispatch('click');
    await waitFor(() => elements.get('status').textContent === 'Opening cancelled');
    assert.equal(canvases().length, 0);
    await pick({ id: 'extract-simple', path: 'corpus/generated/text/extract-simple.pdf' });
    await waitFor(ready);
    const text = () => view(0)?.children.find((n) => n.className === 'text-layer')!;
    assert.equal(
      text()
        .children.map((n) => n.textContent)
        .join(''),
      'ZZ',
    );
    assert.equal(text().getAttribute('role'), 'document');
    assert.equal(text().children[0].style.transform, 'matrix(4,0,0,6,10,214)');
    elements.get('copy-page').dispatch('click');
    await waitFor(() => copied === 'ZZ');
    let copyResult: string | undefined;
    fakeWindow.getSelection = () => ({
      isCollapsed: false,
      rangeCount: 1,
      getRangeAt: () => ({
        startContainer: text().children[0],
        endContainer: text().children[1],
        cloneContents: () => ({ textContent: 'ZZ' }),
      }),
    });
    fakeWindow.dispatch('copy', {
      clipboardData: {
        setData: (type: string, value: string) => {
          assert.equal(type, 'text/plain');
          copyResult = value;
        },
      },
    });
    assert.equal(copyResult, 'ZZ');
    fakeWindow.getSelection = () => null;
    fakeWindow.dispatch('keydown', { key: 'f', ctrlKey: true });
    assert.equal(elements.get('find-panel').hidden, false);
    assert.equal(active(), elements.get('find-query'));
    elements.get('find-query').value = 'z';
    elements.get('find-query').dispatch('input');
    await waitFor(() => elements.get('find-count').textContent === '1 / 2');
    const marks = () => view(0)!.children.find((n) => n.className === 'search-highlights')?.children ?? [];
    assert.equal(marks().length, 2);
    assert.equal(marks()[0].style.left, '10px');
    assert.equal(marks()[0].style.top, '214px');
    elements.get('find-next').dispatch('click');
    await waitFor(() => elements.get('find-count').textContent === '2 / 2');
    assert.equal(marks()[1].className, 'search-match current-match');
    elements.get('find-previous').dispatch('click');
    await waitFor(() => elements.get('find-count').textContent === '1 / 2');
    elements.get('find-case').checked = true;
    elements.get('find-case').dispatch('change');
    await waitFor(() => elements.get('find-count').textContent === '0 / 0');
    elements.get('find-case').checked = false;
    elements.get('find-word').checked = true;
    elements.get('find-word').dispatch('change');
    await waitFor(() => elements.get('find-count').textContent === '0 / 0');
    elements.get('find-word').checked = false;
    elements.get('find-close').dispatch('click');
    await pick({ id: 'outlines', path: 'corpus/generated/document/navigation-outlines.pdf' });
    await waitFor(ready);
    const nodes = () => elements.get('outline-tree').children;
    assert.equal(nodes().length, 3);
    assert.equal(nodes()[0].textContent, 'Root €');
    assert.equal(nodes()[0].getAttribute('aria-expanded'), 'true');
    nodes()[0].dispatch('keydown', { key: 'ArrowDown' });
    assert.equal(active().textContent, 'Child 😀');
    active().dispatch('keydown', { key: 'ArrowRight' });
    assert.equal(nodes().length, 4);
    active().dispatch('keydown', { key: 'ArrowRight' });
    assert.equal(active().textContent, 'Hidden');
    active().dispatch('keydown', { key: 'ArrowLeft' });
    assert.equal(active().textContent, 'Child 😀');
    active().dispatch('keydown', { key: 'End' });
    assert.equal(active().textContent, 'Fin');
    active().dispatch('keydown', { key: 'Enter' });
    await waitFor(() => ready() && elements.get('page').value === '2');
    await pick({ id: 'labels', path: 'corpus/generated/document/navigation-page-labels.pdf' });
    await waitFor(ready);
    assert.equal(elements.get('page-label').textContent, 'Label: Intro');
    elements.get('page').value = 'IX';
    elements.get('page').dispatch('change');
    await waitFor(() => ready() && elements.get('page').value === '4');
    assert.equal(elements.get('page-label').textContent, 'Label: IX');
    elements.get('page').value = '#2';
    elements.get('page').dispatch('change');
    await waitFor(() => ready() && elements.get('page').value === '2');
    assert.equal(elements.get('page-label').textContent, 'Label: A-8');
    await pick({ id: 'links', path: 'corpus/generated/document/navigation-links-actions.pdf' });
    await waitFor(ready);
    const links = view(0)!.children.find((n) => n.className === 'link-layer')!.children;
    assert.equal(links[0].getAttribute('role'), 'link');
    assert.equal(links[0].getAttribute('tabindex'), '0');
    assert.equal(links[0].getAttribute('href'), undefined);
    const linkNames = [
      'Go to page 2',
      'GoToR action (not run)',
      'GoToR action (not run)',
      'Multiple actions (not run)',
      'Go to next page',
      'Go to previous page',
      'Go to first page',
      'Go to last page',
      'Launch action (not run)',
      'JavaScript action (not run)',
      'JavaScript action (not run)',
    ];
    assert.equal(links.length, linkNames.length);
    for (const [i, link] of links.entries()) {
      assert.equal(link.getAttribute('aria-label'), linkNames[i]);
      assert.equal(link.getAttribute('title'), linkNames[i]);
    }
    for (const index of [1, 3, 8, 9, 10]) {
      links[index].dispatch('keydown', { key: 'Enter' });
      assert.ok(elements.get('reading-status').textContent.startsWith(linkNames[index]));
      assert.ok(!elements.get('reading-status').textContent.includes('Error:'));
      assert.equal(elements.get('error').hidden, true);
    }
    links[0].dispatch('keydown', { key: 'Enter' });
    await waitFor(() => ready() && elements.get('page').value === '2');
    elements.get('zoom').value = '1';
    const reading = readingWorkload(2, 1),
      picker = elements.get('file');
    picker.files = [new File([reading.bytes], 'reading.pdf')];
    picker.dispatch('change');
    await waitFor(ready);
    elements.get('find-open').dispatch('click');
    elements.get('find-query').value = 'navigation';
    elements.get('find-query').dispatch('input');
    await waitFor(() => elements.get('find-count').textContent === '1 / 2');
    elements.get('find-next').dispatch('click');
    await waitFor(() => ready() && elements.get('page').value === '2');
    assert.ok(
      view(1)!
        .children.find((n) => n.className === 'search-highlights')!
        .children.some((n) => n.className === 'search-match current-match'),
    );
    elements.get('find-previous').dispatch('click');
    await waitFor(() => ready() && elements.get('page').value === '1');
    elements.get('find-close').dispatch('click');
    let uiConfirmed = 0,
      uiOpened: string[] = [];
    const hostileTitle = '<img src=x onerror=alert(1)>',
      hostileCopy = '<svg onload=alert(1)>';
    const pickSecurity = async (uri: string) => {
      picker.files = [new File([readingSecurityPdf(uri, hostileTitle, hostileCopy)], 'reading-security.pdf')];
      picker.dispatch('change');
      await waitFor(ready);
    };
    await pickSecurity('https://example.invalid/chapter');
    assert.equal(elements.get('outline-tree').children[0].textContent, hostileTitle);
    assert.equal(
      text()
        .children.map((n) => n.textContent)
        .join(''),
      hostileCopy,
    );
    assert.ok(text().children.every((n) => n.tagName === 'SPAN'));
    fakeWindow.confirm = (message) => {
      assert.match(message, /https:\/\/example.invalid\/chapter/);
      uiConfirmed++;
      return false;
    };
    fakeWindow.open = (url, target, features) => {
      assert.equal(target, '_blank');
      assert.equal(features, 'noopener,noreferrer');
      uiOpened.push(url);
    };
    const externalLink = () => view(0)!.children.find((n) => n.className === 'link-layer')!.children[0];
    assert.equal(
      externalLink().getAttribute('aria-label'),
      'Open https://example.invalid/chapter (external; asks before opening)',
    );
    assert.equal(externalLink().getAttribute('title'), externalLink().getAttribute('aria-label'));
    externalLink().dispatch('keydown', { key: 'Enter' });
    assert.equal(uiConfirmed, 1);
    assert.deepEqual(uiOpened, []);
    fakeWindow.confirm = () => {
      uiConfirmed++;
      return true;
    };
    externalLink().dispatch('click');
    assert.equal(uiConfirmed, 2);
    assert.deepEqual(uiOpened, ['https://example.invalid/chapter']);
    for (const uri of ['javascript:alert(1)', 'data:text/html,test', 'file:///private/test']) {
      await pickSecurity(uri);
      externalLink().dispatch('keydown', { key: 'Enter' });
      assert.match(elements.get('reading-status').textContent, /Blocked URI scheme/);
    }
    assert.equal(uiConfirmed, 2);
    assert.equal(uiOpened.length, 1);
    for (const [requested, expected] of [
      [0.01, 0.1],
      [100, 8],
    ]) {
      picker.files = [
        new File(
          [readingSecurityPdf('', hostileTitle, hostileCopy, { zoom: requested, label: hostileTitle })],
          'destination.pdf',
        ),
      ];
      picker.dispatch('change');
      await waitFor(ready);
      const link = externalLink(),
        name = `Go to page ${hostileTitle}`;
      assert.equal(link.getAttribute('aria-label'), name);
      assert.equal(link.getAttribute('title'), name);
      assert.equal(link.children.length, 0, 'PDF markup in an accessible name stays an inert attribute');
      link.dispatch('keydown', { key: 'Enter' });
      await waitFor(() => ready() && view(0)!.style.width === `${64 * expected}px`);
      assert.equal(
        elements.get('reading-status').textContent,
        `Destination zoom limited to ${expected * 100}% (viewer range: 10%–800%).`,
      );
      assert.equal(elements.get('error').hidden, true);
      assert.equal(elements.get('page').value, '1');
      assert.equal(elements.get('viewport').scrollTop, 8 * expected);
    }
    // Confirm policy and inert markup use the same production UI modules.
    let confirmed = 0,
      opened: string[] = [],
      reported: string[] = [],
      notices: string[] = [];
    const action = { kind: 2, uri: '../chapter', base: 'https://example.invalid/docs/', isMap: false } as UriAction;
    const options = {
      destination: async () => {},
      named: async () => {},
      report: (v: unknown) => reported.push(String(v)),
      notice: (v: string) => notices.push(v),
      confirm: () => {
        confirmed++;
        return false;
      },
      open: (v: string) => opened.push(v),
    };
    await activateTarget([action], options);
    assert.equal(confirmed, 1);
    assert.deepEqual(opened, []);
    await activateTarget([action], {
      ...options,
      confirm: (value) => {
        assert.match(value, /https:\/\/example.invalid\/chapter/);
        return true;
      },
    });
    assert.deepEqual(opened, ['https://example.invalid/chapter']);
    for (const uri of ['javascript:alert(1)', 'data:text/html,test', 'file:///private/test']) {
      assert.throws(() => externalUrl({ ...action, uri }), /Blocked/);
      await activateTarget([{ ...action, uri }], options);
    }
    assert.equal(confirmed, 1);
    assert.equal(opened.length, 1);
    assert.equal(reported.length, 3);
    for (const target of [
      [],
      [action, action],
      [{ ...action, isMap: true }],
      ...[4, 5, 6].map((kind) => [{ kind }]),
      ...['Unknown', 'constructor', '__proto__'].map((name) => [{ kind: 3, name }]),
    ] as Target[]) {
      await activateTarget(target, {
        ...options,
        named: () => assert.fail('Unsupported named action must stay inert'),
      });
    }
    assert.equal(notices.length, 9);
    assert.ok(notices.every((v) => !v.includes('Error:')));
    assert.equal(reported.length, 3);
    assert.equal(opened.length, 1);
    assert.equal(confirmed, 1);
    assert.equal(targetLabel([{ kind: 1, page: 2 }] as Target), 'Go to page 3');
    assert.equal(
      targetLabel([{ kind: 1, page: 2 }] as Target, () => 'iii'),
      'Go to page iii',
    );
    const hostile = '<img src=x onerror=alert(1)>';
    const rootElement = new Element('tree');
    const tree = new OutlineTree(rootElement as unknown as HTMLElement, () => {});
    tree.set([{ depth: 0, title: hostile, open: false, style: 0, color: [0, 0, 0], target: [] }]);
    assert.equal(rootElement.children[0].getAttribute('aria-label'), undefined);
    assert.equal(rootElement.children[0].textContent, hostile);
    const maliciousText = '<svg onload=alert(1)>';
    let byte = 0;
    const characters = [...maliciousText].map((c) => ({ start: byte, end: ++byte, quadStart: 0, quadCount: 0 }));
    // createElement returns the stand-in Element.
    const inertLayer = textLayer(
      { text: maliciousText, quads: [], characters },
      { crop: [0, 0, 64, 64], userUnit: 1, rotation: 0 },
      1,
    ) as unknown as Element;
    assert.equal(inertLayer.children.map((n) => n.textContent).join(''), maliciousText);
    assert.ok(inertLayer.children.every((n) => n.tagName === 'SPAN'));
    for (const [rotation, expected] of [
      [0, [20, 80]],
      [90, [20, 20]],
      [180, [80, 20]],
      [270, [80, 80]],
    ] as [number, number[]][])
      assert.deepEqual(pagePoint({ crop: [10, 20, 60, 70], userUnit: 2, rotation }, 1, 20, 30), expected);
    const geometry = { crop: [0, 0, 100, 200], width: 100, height: 200, userUnit: 1, rotation: 0 };
    assert.deepEqual(destinationView({ fit: 0, coordinates: [12, 150, 2, null] }, geometry, 200, 400, 1), {
      zoom: 2,
      x: 24,
      y: 100,
    });
    for (const value of [null, 0])
      assert.deepEqual(destinationView({ fit: 0, coordinates: [null, null, value, null] }, geometry, 200, 400, 2), {
        zoom: 2,
        x: null,
        y: null,
      });
    assert.deepEqual(destinationView({ fit: 4, coordinates: [10, 20, 50, 100] }, geometry, 200, 400, 1), {
      zoom: 5,
      x: 50,
      y: 500,
    });
    for (const [requested, zoom] of [
      [0.01, 0.1],
      [100, 8],
    ]) {
      const plan = destinationView({ fit: 0, coordinates: [12, 150, requested, null] }, geometry, 200, 400, 1);
      assert.deepEqual(plan, {
        zoom,
        x: 12 * zoom,
        y: 50 * zoom,
        notice: `Destination zoom limited to ${zoom * 100}% (viewer range: 10%–800%).`,
      });
    }
    for (const fit of [1, 2, 3, 4]) {
      const plan = destinationView({ fit, coordinates: [10, 20, 50, 100] }, geometry, 10000, 20000, 1);
      assert.equal(plan.zoom, 8);
      assert.match(plan.notice!, /800%/);
      if (fit === 4) {
        assert.equal(plan.x, 80);
        assert.equal(plan.y, 800);
      }
      const small = destinationView({ fit, coordinates: [10, 20, 50, 100] }, geometry, 1, 1, 1);
      assert.equal(small.zoom, 0.1);
      assert.match(small.notice!, /10%/);
    }
    const painted = [10, 20, 50, 100];
    assert.deepEqual(
      destinationView({ fit: 5, coordinates: [null, null, null, null] }, geometry, 200, 400, 1, {}, painted),
      { zoom: 5, x: 50, y: 500 },
    );
    assert.deepEqual(
      destinationView({ fit: 6, coordinates: [80, null, null, null] }, geometry, 200, 400, 1, {}, painted),
      { zoom: 5, x: 50, y: 600 },
    );
    assert.deepEqual(
      destinationView({ fit: 7, coordinates: [15, null, null, null] }, geometry, 200, 400, 1, {}, painted),
      { zoom: 5, x: 75, y: 500 },
    );
    for (const [fit, expected] of [
      [5, { zoom: 2, x: 0, y: 0 }],
      [6, { zoom: 2, x: 0, y: 240 }],
      [7, { zoom: 2, x: 160, y: 0 }],
    ] as [number, object][]) {
      assert.deepEqual(
        destinationView({ fit, coordinates: [80, null, null, null] }, geometry, 200, 400, 1, {}, null),
        expected,
      );
    }
    assert.throws(
      () => destinationView({ fit: 0, coordinates: [null, null, Infinity, null] }, geometry, 200, 400, 1),
      /Invalid destination zoom/,
    );

    elements.get('zoom').value = '1';
    elements.get('viewport').clientHeight = 300;
    picker.files = [new File([navigationDocument()], 'content-fits.pdf')];
    picker.dispatch('change');
    await waitFor(() => ready() && elements.get('outline-tree').children.length === 6);
    for (let index = 0; index < 6; index++) {
      const target = index < 3 ? 1 : 2,
        width = index < 3 ? 120 : 600,
        height = index < 3 ? 200 : 800;
      const expected =
        index % 3 === 0 ? Math.min(252 / width, 252 / height) : index % 3 === 1 ? 252 / width : 252 / height;
      elements.get('outline-tree').children[index].dispatch('keydown', { key: 'Enter' });
      await waitFor(
        () =>
          ready() &&
          elements.get('page').value === String(target + 1) &&
          view(target)?.style.width === `${600 * expected}px`,
      );
      assert.equal(elements.get('error').hidden, true);
      assert.equal(elements.get('page-label').textContent, `Label: ${target === 1 ? 'Painted' : 'Empty'}`);
      // Model the browser's queued scroll event after programmatic navigation.
      elements.get('viewport').dispatch('scroll');
      await waitFor(ready);
      assert.equal(elements.get('page').value, String(target + 1));
    }
    elements.get('zoom').value = '1';
    elements.get('zoom').dispatch('change');
    elements.get('page').value = '2';
    elements.get('page').dispatch('change');
    await waitFor(() => ready() && view(1));
    elements.get('viewport').scrollTop = 824 - 8; // 800px page and 24px inter-page gap.
    elements.get('viewport').dispatch('scroll');
    await waitFor(ready);
    assert.equal(elements.get('page').value, '2');

    // Labels can arrive after visible links. Refresh names without adding a
    // second tab stop for a multiline link, and announce the updated name.
    let resolveLabels!: (labels: string[]) => void;
    const lateClient = {
      invalidateSearch() {},
      labels: () =>
        new Promise<string[]>((resolve) => {
          resolveLabels = resolve;
        }),
      outlines: async () => [],
      pageText: async () => ({ text: '', characters: [], quads: [] }),
      links: async () => [
        {
          target: [{ kind: 1, page: 0 }],
          quads: [
            [1, 2, 3, 2, 3, 4, 1, 4],
            [5, 6, 7, 6, 7, 8, 5, 8],
          ],
        },
      ],
    };
    const late = new ReadingView({
      // The stand-in session needs no scrollMatch: nothing scrolls to a search match here.
      state: () =>
        ({
          client: lateClient as unknown as SessionClient,
          handle: 1,
          epoch: 1,
          page: 0,
          count: 1,
          zoom: 1,
        }) as ReadingState,
      navigate: async () => {},
      destination: async () => {},
      report: (error) => assert.fail(String(error)),
      notice: (message) => {
        elements.get('reading-status').textContent = message as string;
      },
      controls: () => {},
    });
    elements.get('find-query').value = '';
    const opening = late.open(),
      latePage = new Element('section');
    await late.page(0, geometry, latePage as unknown as HTMLElement);
    const areas = latePage.children.find((n) => n.className === 'link-layer')!.children;
    assert.equal(areas[0].getAttribute('aria-label'), 'Go to page 1');
    resolveLabels(['iii']);
    await opening;
    for (const area of areas) {
      assert.equal(area.getAttribute('aria-label'), 'Go to page iii');
      assert.equal(area.getAttribute('title'), 'Go to page iii');
    }
    assert.equal(areas[0].getAttribute('tabindex'), '0');
    assert.equal(areas[1].getAttribute('tabindex'), '-1');
    assert.equal(areas[1].getAttribute('aria-hidden'), 'true');
    areas[0].dispatch('focus');
    assert.equal(elements.get('reading-status').textContent, 'Go to page iii');
    late.reset();
  } finally {
    fakeWindow.dispatch('pagehide');
    assert.equal(disconnected, true);
    for (const [key, descriptor] of original) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete (globalThis as Record<string, unknown>)[key];
    }
  }
}

async function testFixtureLoading() {
  const base = 'http://localhost:8000/';
  assert.equal(fixtureUrl('/fixtures/classic/two-pages.pdf', base).href, `${base}fixtures/classic/two-pages.pdf`);
  assert.equal(fixtureUrl(`${base}fixtures/classic/two-pages.pdf`, base).origin, new URL(base).origin);
  const invalid = [
    '',
    '/viewer.mjs',
    '/fixtures-other/file.pdf',
    '/fixtures/',
    'https://example.invalid/fixtures/file.pdf',
    '//example.invalid/fixtures/file.pdf',
    'file:///fixtures/file.pdf',
    'data:application/pdf,test',
    'javascript:alert(1)',
    '/fixtures/../Cargo.toml',
    '/fixtures/classic/../two-pages.pdf',
    '/fixtures/%2e%2e/file.pdf',
    '/fixtures/classic/%2e%2e%2f..%2fsecret.pdf',
    '/fixtures/classic/%5cfile.pdf',
    '/fixtures/%00.pdf',
    '/fixtures/file.pdf?query=1',
    '/fixtures/file.pdf#fragment',
    'http://user:password@localhost:8000/fixtures/file.pdf',
    ' /fixtures/file.pdf',
  ];
  const originalFetch = globalThis.fetch;
  try {
    let requests = 0;
    globalThis.fetch = async () => {
      requests++;
      assert.fail('Invalid startup URL reached fetch');
    };
    for (const value of invalid) {
      assert.throws(() => fixtureUrl(value, base));
      await assert.rejects(loadFixture(value, base));
    }
    assert.equal(requests, 0);
    globalThis.fetch = async (url, options) => {
      assert.equal(url, `${base}fixtures/classic/two-pages.pdf`);
      assert.deepEqual(options, { mode: 'same-origin', credentials: 'omit', redirect: 'error' });
      return new Response(Uint8Array.from([37, 80, 68, 70]), { headers: { 'Content-Length': '4' } });
    };
    const file = await loadFixture('/fixtures/classic/two-pages.pdf', base);
    assert.equal(file.name, 'two-pages.pdf');
    bytesEqual(new Uint8Array(await file.arrayBuffer()), [37, 80, 68, 70], 'fixture loader bytes');
    globalThis.fetch = async () => new Response('', { status: 404 });
    await assert.rejects(loadFixture('/fixtures/classic/two-pages.pdf', base), /HTTP 404.*--fixtures/);
    globalThis.fetch = async () => new Response('small', { headers: { 'Content-Length': String(MAX_FILE + 1) } });
    await assert.rejects(loadFixture('/fixtures/classic/two-pages.pdf', base), /input budget/);
    let cancelled = false,
      released = false;
    globalThis.fetch = async () =>
      ({
        ok: true,
        headers: new Headers(),
        body: {
          getReader: () => ({
            // Model an over-budget stream chunk without allocating 512 MiB.
            read: async () => ({ done: false, value: { byteLength: MAX_FILE + 1 } }),
            cancel: async () => {
              cancelled = true;
            },
            releaseLock: () => {
              released = true;
            },
          }),
        },
      }) as unknown as Response;
    await assert.rejects(loadFixture('/fixtures/classic/two-pages.pdf', base), /input budget/);
    assert.equal(cancelled, true);
    assert.equal(released, true);
  } finally {
    globalThis.fetch = originalFetch;
  }
}

// Minimal independent security PDF (7.5.4, 12.3.3, 12.5.6.5, 14.9.4).
function readingSecurityPdf(
  uri: string,
  title: string,
  text: string,
  { zoom = null, label = null }: { zoom?: number | null; label?: string | null } = {},
) {
  const hex = (s: string) => `<${Buffer.from(s, 'utf8').toString('hex')}>`;
  const content = `/Span << /ActualText ${hex(text)} >> BDC 0 0 10 10 re f EMC\n`;
  const objects = [
    null,
    `<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R ${label === null ? '' : `/PageLabels << /Nums [0 << /P ${hex(label)} >>] >>`} >>`,
    '<< /Type /Pages /Count 1 /Kids [3 0 R] >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << >> /Annots [4 0 R] /Contents 7 0 R >>',
    `<< /Type /Annot /Subtype /Link /Rect [8 8 24 24] ${zoom === null ? `/A << /S /URI /URI ${hex(uri)} >>` : `/Dest [3 0 R /XYZ 8 56 ${zoom}]`} >>`,
    '<< /Type /Outlines /Count 1 /First 6 0 R /Last 6 0 R >>',
    `<< /Title ${hex(title)} /Parent 5 0 R /Dest [3 0 R /Fit] >>`,
    `<< /Length ${Buffer.byteLength(content)} >>\nstream\n${content}endstream`,
  ];
  let pdf = '%PDF-2.0\n',
    offsets: number[] = [];
  for (let i = 1; i < objects.length; i++) {
    offsets.push(pdf.length);
    pdf += `${i} 0 obj\n${objects[i]}\nendobj\n`;
  }
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length}\n0000000000 65535 f \n${offsets.map((n) => `${String(n).padStart(10, '0')} 00000 n \n`).join('')}trailer\n<< /Root 1 0 R /Size ${objects.length} >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(pdf);
}
// Fixture vector syntax is independently generated, never parsed by Pagura here.
type Vector = (Vector | Buffer | boolean | number)[];
function fixtureVector(source: string) {
  const stack: Vector[] = [];
  let root: Vector | undefined;
  for (const token of source.match(/\[|\]|<[^>]*>|true|false|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?/g) ?? []) {
    if (token === '[') {
      const value: Vector = [];
      if (stack.length) stack.at(-1)!.push(value);
      else root = value;
      stack.push(value);
    } else if (token === ']') stack.pop();
    else
      stack
        .at(-1)!
        .push(
          token[0] === '<'
            ? Buffer.from(token.slice(1, -1), 'hex')
            : token === 'true'
              ? true
              : token === 'false'
                ? false
                : Number(token),
        );
  }
  assert.equal(stack.length, 0);
  return root as Vector;
}
async function testReading(client: SessionClient, root: string, native: string, wasiRun: WasiRun) {
  const parity = async (name: string, requests: RequestTuple[], version = VERSION_1_2) => {
    const input = Buffer.concat([
      encodeRequest(1, 1, { version }),
      ...requests.map(([kind, values], i) => encodeRequest(kind, i + 2, values)),
    ]);
    const result = spawnSync(native, [], { input, timeout: 30000, maxBuffer: 64 * 1024 * 1024 });
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr?.toString());
    const wasi = await wasiRun(input, `reading-${name}`);
    assert.equal(wasi.code, 0);
    // These small fixtures fit both targets' byte budgets. Capacity accounting
    // uses target allocation sizes, so only its eight text-byte bytes may
    // differ here; entry counts and every document result/layout stay exact.
    const accounted = (bytes: Uint8Array) => {
      const copy = Buffer.from(bytes);
      let offset = 0;
      for (const frame of parse(bytes)) {
        const length = copy.readUInt32LE(offset);
        if (frame.kind === 143) {
          assert.equal(frame.value.textBytes === 0, frame.value.textPages === 0);
          assert.ok(frame.value.textBytes <= 16 * 1024 * 1024);
          copy.fill(0, offset + 4 + 29, offset + 4 + 37);
        }
        offset += 4 + length;
      }
      return copy;
    };
    bytesEqual(accounted(wasi.bytes), accounted(result.stdout), `reading ${name} native/WASI identity`);
    return parse(wasi.bytes);
  };
  const open = (bytes: Uint8Array, recovery = 1): RequestTuple => [2, { bytes, password: new Uint8Array(), recovery }];
  const navigation = navigationDocument();
  const bounds = await parity(
    'content-bounds',
    [
      open(navigation),
      ...[0, 1, 2].map((page): RequestTuple => [16, { document: 1, page }]),
      [16, { document: 1, page: 99 }],
      [16, { document: 99, page: 0 }],
    ],
    VERSION_1_3,
  );
  assert.deepEqual(
    bounds.slice(2, 5).map((f) => f.value.bounds),
    [[20, 480, 320, 740], [60, 100, 180, 300], null],
  );
  assert.ok(bounds.slice(5).every((f) => f.kind === 255 && f.value.category === 7));
  for (const version of [1, VERSION_1_1, VERSION_1_2]) {
    const older = await parity(
      `bounds-older-${version}`,
      [open(navigation), [16, { document: 1, page: 1 }], [3, { document: 1, page: 1 }]],
      version,
    );
    assert.equal(older[2].value.category, 7);
    assert.equal(older[3].kind, 131);
  }
  const heavy = readingWorkload(4, 50),
    heavyDoc = await client.open(heavy.bytes);
  for (let page = 0; page < 4; page++) await client.pageText(heavyDoc.document, page);
  const heavyStats = await client.readingStats();
  assert.equal(heavyStats.textPages, 4);
  assert.ok(heavyStats.textBytes < 16 * 1024 * 1024);
  await client.close(heavyDoc.document);
  const manifest: Manifest = JSON.parse(await readFile(join(root, 'corpus/manifests/text.json'), 'utf8'));
  for (const entry of manifest.cases.filter((entry) => entry.id.startsWith('text/extract-'))) {
    const pdf = new Uint8Array(await readFile(join(root, entry.files['.pdf'].path)));
    const expected = await readFile(join(root, entry.files['.text'].path), 'utf8');
    // Each query: bytes, ignore case, whole word, fold, expected [start, end] ranges, glyph indexes per match.
    const queries = fixtureVector(await readFile(join(root, entry.files['.queries'].path), 'utf8')) as [
      Buffer,
      boolean,
      boolean,
      boolean,
      number[][],
      number[][],
    ][];
    // Each glyph row carries its eight quad coordinates at indexes 1-8.
    const glyphs = fixtureVector(await readFile(join(root, entry.files['.glyphs'].path), 'utf8')) as number[][];
    const queryRequests = queries.map(
      (q): RequestTuple => [
        11,
        {
          document: 1,
          start: 0,
          end: 1,
          offset: 0,
          limit: 1024,
          query: q[0].toString('utf8'),
          flags: (q[1] ? 0 : 1) | (q[2] ? 2 : 0) | (q[3] ? 4 : 0),
        },
      ],
    );
    const frames = await parity(entry.id.split('/')[1], [
      open(pdf),
      [10, { document: 1, page: 0 }],
      ...queryRequests,
      [15],
      [6, { document: 1 }],
      [15],
    ]);
    for (const [i, query] of queries.entries()) {
      const response = frames[i + 3];
      assert.equal(response.kind, 139, String(response.value));
      assert.deepEqual(
        response.value.matches.map((m: Match) => [m.start, m.end]),
        query[4],
      );
      assert.deepEqual(
        response.value.matches.map((m: Match) => m.quads),
        query[5].map((indexes) => indexes.map((index) => glyphs[index].slice(1, 9))),
      );
    }
    assert.equal(frames[2].kind, 138, String(frames[2].value));
    assert.equal(frames[2].value.text, expected);
    const opened = await client.open(pdf),
      text = await client.pageText(opened.document, 0);
    assert.equal(text.text, expected);
    assert.deepEqual(text, frames[2].value);
    assert.equal(text.characters.length, [...expected].length);
    if (entry.id === 'text/extract-simple') {
      assert.deepEqual(
        text.characters.map((c) => [c.start, c.end, text.quads.slice(c.quadStart, c.quadStart + c.quadCount)]),
        [
          [0, 1, [[10, 20, 14, 20, 14, 26, 10, 26]]],
          [1, 2, [[14, 20, 18, 20, 18, 26, 14, 26]]],
        ],
      );
      const results: Match[] = [];
      let offset = 0,
        page = 0;
      do {
        // Without a token, searches are never cancelled.
        const result = (await client.search(opened.document, 'z', page, 1, offset, 1, 4)) as SearchResult;
        results.push(...result.matches);
        page = result.nextPage;
        offset = result.nextOffset;
      } while (page < 1);
      assert.deepEqual(
        results.map((m) => [m.page, m.start, m.end]),
        [
          [0, 0, 1],
          [0, 1, 2],
        ],
      );
      assert.equal(((await client.search(opened.document, 'z', 0, 1, 0, 128, 5)) as SearchResult).matches.length, 0);
      assert.equal(((await client.search(opened.document, 'Z', 0, 1, 0, 128, 6)) as SearchResult).matches.length, 0);
      const searchFrames = await parity('search', [
        open(pdf),
        [11, { document: 1, query: 'z', start: 0, end: 1, offset: 0, limit: 1, flags: 4 }],
        [11, { document: 1, query: 'z', start: 0, end: 1, offset: 1, limit: 1, flags: 4 }],
      ]);
      assert.equal(searchFrames[2].value.nextOffset, 1);
      assert.equal(searchFrames[3].value.done, true);
    }
    assert.ok(frames.at(-3)!.value.textBytes > 0);
    assert.equal(frames.at(-3)!.value.textPages, 1);
    assert.equal(frames.at(-1)!.value.textBytes, 0);
    assert.equal(frames.at(-1)!.value.textPages, 0);
    await client.close(opened.document);
  }
  for (const [name, kind] of [
    ['navigation-outlines', 13],
    ['navigation-links-actions', 12],
    ['navigation-links-quads', 12],
    ['navigation-page-labels', 14],
  ] as [string, number][]) {
    const bytes = new Uint8Array(await readFile(join(root, `corpus/generated/document/${name}.pdf`)));
    const frames = await parity(name, [open(bytes), [kind, { document: 1, page: 0 }]]);
    assert.equal(frames[2].kind, kind + 128, String(frames[2].value));
    if (kind === 13) {
      assert.deepEqual(
        frames[2].value.map((v: OutlineItem) => [v.depth, v.title, v.open, v.style]),
        [
          [0, 'Root €', true, 3],
          [1, 'Child 😀', false, 0],
          [2, 'Hidden', false, 0],
          [0, 'Fin', false, 0],
        ],
      );
      assert.deepEqual(frames[2].value[0].color, [0.2, 0.3, 0.4]);
      assert.deepEqual(frames[2].value[0].target[0], {
        parent: 0,
        kind: 1,
        page: 0,
        fit: 1,
        coordinates: [null, null, null, null],
      });
    }
    if (kind === 14)
      assert.deepEqual(
        frames[2].value,
        (await readFile(join(root, `corpus/generated/document/${name}.expected`), 'utf8')).trimEnd().split('\n'),
      );
    if (kind === 12) {
      const links = frames[2].value;
      assert.deepEqual(
        links.map((v: Link) => v.target[0].kind),
        [1, 4, 4, 2, 3, 3, 3, 3, 5, 6, 6],
      );
      assert.deepEqual(links[0].quads, [[1, 2, 31, 2, 31, 42, 1, 42]]);
      assert.equal(links[3].target.length, 4);
      assert.equal(links[3].target[0].uri, '../chapter');
      assert.equal(links[3].target[0].base, 'https://example.invalid/docs/');
    }
  }
  const workload = readingWorkload(10, 1),
    long = await client.open(workload.bytes);
  for (let page = 0; page < 10; page++) assert.equal((await client.pageText(long.document, page)).text, workload.text);
  const cached = await client.readingStats();
  assert.equal(cached.textPages, 8);
  await client.pageText(long.document, 9);
  assert.deepEqual(await client.readingStats(), cached);
  await client.close(long.document);
  assert.equal((await client.readingStats()).textBytes, 0);
  const simple = new Uint8Array(await readFile(join(root, 'corpus/generated/text/extract-simple.pdf')));
  const invalid: RequestTuple[] = [
    [10, { document: 99, page: 0 }],
    [12, { document: 1, page: 99 }],
    [13, { document: 99 }],
    [14, { document: 99 }],
    ...[{ start: 0, end: 17 }, { start: 1, end: 0 }, { limit: 0 }, { offset: 99 }, { query: '' }].map(
      (overrides): RequestTuple => [
        11,
        { document: 1, query: 'Z', start: 0, end: 1, offset: 0, limit: 1, flags: 4, ...overrides },
      ],
    ),
  ];
  const errors = await parity('errors', [open(simple), ...invalid]);
  for (const frame of errors.slice(2)) {
    assert.equal(frame.kind, 255);
    assert.equal(frame.value.category, 7);
  }
  for (const [name, kind, repair] of [
    ['navigation-outlines-cycle', 13, 'outline.cycle'],
    ['navigation-links-quads', 12, 'link.quad-points'],
    ['navigation-action-cycle', 12, 'action.cycle'],
  ] as [string, number, string][]) {
    const bytes = new Uint8Array(await readFile(join(root, `corpus/generated/document/${name}.pdf`)));
    const frames = await parity(`strict-${name}`, [open(bytes, 0), [kind, { document: 1, page: 0 }]]);
    assert.equal(frames[2].kind, 255);
    assert.equal(frames[2].value.category, 1);
    assert.equal(frames[2].value.repair, repair);
  }
  for (const version of [1, VERSION_1_1]) {
    const frames = await parity(
      `older-${version}`,
      [
        open(simple),
        ...[10, 11, 12, 13, 14, 15].map(
          (kind): RequestTuple => [
            kind,
            { document: 1, page: 0, query: 'Z', start: 0, end: 1, offset: 0, limit: 1, flags: 4 },
          ],
        ),
        [3, { document: 1, page: 0 }],
      ],
      version,
    );
    assert.ok(frames.slice(2, -1).every((f) => f.kind === 255 && f.value.category === 7));
    assert.equal(frames.at(-1)!.kind, 131);
  }
}

async function testDiagnosticKinds(client: SessionClient, root: string, native: string, wasiRun: WasiRun) {
  const bytes = new Uint8Array(await readFile(join(root, 'corpus/generated/paint/bundled-diagnostic-kinds.pdf')));
  const rect = { x: 0, y: 0, width: 64, height: 64 };
  const tile = { document: 1, page: 0, scale: 1, ...rect };
  for (const version of [1, VERSION_1_1, VERSION_1_2, VERSION_1_3, VERSION_1_4]) {
    const input = Buffer.concat([
      encodeRequest(1, 1, { version }),
      encodeRequest(2, 2, { bytes, recovery: 1, password: new Uint8Array() }),
      encodeRequest(4, 3, tile),
      encodeRequest(5, 4, { document: 1 }),
      encodeRequest(17, 5, { document: 1 }),
      encodeRequest(6, 6, { document: 1 }),
    ]);
    const output = spawnSync(native, [], { input, timeout: 30000 });
    assert.ifError(output.error);
    assert.equal(output.status, 0);
    const wasi = await wasiRun(input, `diagnostic-kinds-${version}`);
    assert.equal(wasi.code, 0);
    bytesEqual(wasi.bytes, output.stdout, 'diagnostic kinds native/WASI parity');
    const frames = parse(wasi.bytes);
    assert.deepEqual(
      frames[3].value.map(({ kind, tag }: Diagnostic) => ({ kind, tag })),
      [{ kind: 'repair', tag: 'font.descriptor-weight' }],
    );
    if (version === VERSION_1_4) {
      assert.equal(frames[4].kind, 145);
      assert.deepEqual(
        frames[4].value.map(({ kind, tag }: Diagnostic) => ({ kind, tag })),
        [
          { kind: 'repair', tag: 'font.descriptor-weight' },
          { kind: 'information', tag: 'font.substituted' },
        ],
      );
    } else {
      assert.equal(frames[4].kind, 255);
      assert.equal(frames[4].value.category, 7);
    }
    assert.equal(frames[5].kind, 134);
  }
  const opened = await client.open(bytes);
  await client.renderTile(opened.document, 0, 1, rect);
  assert.deepEqual(
    (await client.diagnostics(opened.document)).map((item) => item.kind),
    ['repair', 'information'],
  );
  await client.close(opened.document);
  const valid = new Uint8Array(await readFile(join(root, 'corpus/generated/paint/bundled-helvetica.pdf')));
  const strict = await client.open(valid, 0);
  await client.renderTile(strict.document, 0, 1, rect);
  const information = await client.diagnostics(strict.document);
  assert.deepEqual(
    information.map(({ kind, tag }) => ({ kind, tag })),
    [{ kind: 'information', tag: 'font.substituted' }],
  );
  await client.close(strict.document);
  // Reject unknown kind bytes and every truncation independently of the encoder.
  const body = Uint8Array.from([145, 1, 0, 0, 0, 1, 0, 0, 0, 1, 3, 0, 0, 0, 97, 46, 98, 1, 0, 0, 0, 120, 0, 0]);
  assert.equal((decodeResponse(body).value as Diagnostic[])[0].kind, 'information');
  for (let end = 0; end < body.length; end++) assert.throws(() => decodeResponse(body.subarray(0, end)));
  const invalid = body.slice();
  invalid[9] = 2;
  assert.throws(() => decodeResponse(invalid), /Unknown diagnostic kind/);
}
