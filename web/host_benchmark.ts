/** Exploratory session measurements; no release or cross-engine speed claim.
 * node web/host_benchmark.ts WASM_PATH EXTERNAL_MULTIPAGE_PDF > /tmp/host.json
 * Inputs are read in place into memory; no document/request files are written.
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { cpus, release } from 'node:os';
import { createHash } from 'node:crypto';
import { WASI } from 'node:wasi';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';
import { SessionClient } from './client.mjs';
import { FrameDecoder, encodeRequest, HostError, VERSION_1_1 } from './protocol.mjs';
import { runHost } from './worker.mjs';
import { failureReason } from './failure.mjs';
import type { Responses } from './client.mjs';
import type { RequestValues, Tile } from './protocol.mjs';

interface Doc {
  name: string;
  bytes: Uint8Array;
  /** Independent PPM pixels; absent for the external document. */
  rgb?: Uint8Array;
  width?: number;
  outputs: string[];
  tileSize: number;
  external?: boolean;
  pages?: number;
}
interface Request {
  kind: keyof Responses;
  values?: RequestValues;
}
interface Reply<T = unknown> {
  value: T;
  ms: number;
  wasmBytes: number;
}
/** The workload yields requests and receives each reply; the first reply is undefined. */
type Driver = Generator<Request, void, Reply | undefined>;
/** A fresh-instance timing record, or one runtime's latency and memory series. */
interface Results {
  start?: number;
  firstMs?: number;
  outcome?: string;
  latency?: object[];
  memory?: object[];
}
type Listener = (event: unknown) => void;

const median = (values: number[]) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted.length % 2
    ? sorted[(sorted.length - 1) / 2]
    : (sorted[sorted.length / 2 - 1] + sorted[sorted.length / 2]) / 2;
};
const hash = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');

/** The same request workload drives both the Node WASI and browser mailbox paths. */
function* workload(documents: Doc[], results: Results, phase: string): Driver {
  yield { kind: 1, values: { version: VERSION_1_1 } };
  const request = function* <K extends keyof Responses>(
    kind: K,
    values?: RequestValues,
  ): Generator<Request, Reply<Responses[K] | HostError>, Reply | undefined> {
    return (yield { kind, values }) as Reply<Responses[K] | HostError>;
  };
  const open = function* (doc: Doc) {
    return yield* request(2, { bytes: doc.bytes, password: new Uint8Array(), recovery: 1 });
  };
  const tile = function* (doc: Doc, document: number, scale: number, x = 0, y = 0) {
    return yield* request(4, { document, page: 0, scale, x, y, width: doc.tileSize, height: doc.tileSize });
  };
  if (phase === 'fresh') {
    const doc = documents[0],
      opened = yield* open(doc);
    assert.ok(!(opened.value instanceof Error));
    yield* request(3, { document: opened.value.document, page: 0 });
    const image = yield* tile(doc, opened.value.document, 1);
    results.firstMs = performance.now() - results.start!;
    results.outcome = image.value instanceof Error ? String(image.value) : 'tile';
    verify(doc, image.value, 1, 0, 0);
    yield* request(6, { document: opened.value.document });
    return;
  }
  if (phase === 'latency')
    for (const doc of documents) {
      const samples: {
        firstMs: number;
        outcome: string;
        warmMs: number[];
        zoomMs: number[];
        [key: string]: unknown;
      }[] = [];
      // Two discarded warmups, five measured samples, within one long-lived instance.
      for (let sample = -2; sample < 5; sample++) {
        const begin = performance.now(),
          opened = yield* open(doc);
        assert.ok(!(opened.value instanceof Error));
        if (doc.external) assert.ok(opened.value.pages > 1, 'External document must have multiple pages');
        doc.pages = opened.value.pages;
        const info = yield* request(3, { document: opened.value.document, page: 0 });
        assert.ok(!(info.value instanceof Error));
        const first = yield* tile(doc, opened.value.document, 1);
        const firstMs = performance.now() - begin;
        verify(doc, first.value, 1, 0, 0);
        const warm: number[] = [],
          zoom: number[] = [];
        for (let i = 0; i < 16; i++) {
          const x = (i % 2) * doc.tileSize,
            y = (Math.floor(i / 2) % 2) * doc.tileSize;
          const result = yield* tile(doc, opened.value.document, 1, x, y);
          warm.push(result.ms);
          verify(doc, result.value, 1, x, y);
        }
        for (const scale of [0.5, 1, 1.5, 2, 1]) {
          const result = yield* tile(doc, opened.value.document, scale);
          zoom.push(result.ms);
          verify(doc, result.value, scale, 0, 0);
        }
        const retained = yield* request(9);
        yield* request(6, { document: opened.value.document });
        if (sample >= 0)
          samples.push({
            firstMs,
            outcome: first.value instanceof Error ? String(first.value) : 'tile',
            warmMs: warm,
            zoomMs: zoom,
            retained: retained.value,
            wasmBytes: retained.wasmBytes,
          });
      }
      results.latency!.push({
        document: doc.name,
        pages: doc.pages,
        tileSize: doc.tileSize,
        samples,
        medians: {
          firstMs: median(samples.map((s) => s.firstMs)),
          warmMs: median(samples.flatMap((s) => s.warmMs)),
          zoomMs: median(samples.flatMap((s) => s.zoomMs)),
        },
      });
    }
  // Fresh baseline for each series; the instance remains alive across all cycles.
  if (phase !== 'latency')
    for (const mode of [phase]) {
      const before = yield* request(9),
        cycles: { wasmBytes: number; [key: string]: unknown }[] = [];
      for (let cycle = 0; cycle < 50; cycle++) {
        const doc = mode === 'same' ? documents[0] : documents[cycle % documents.length];
        const opened = yield* open(doc);
        const rendered = yield* tile(doc, (opened.value as Responses[2]).document, 1);
        verify(doc, rendered.value, 1, 0, 0);
        const retained = yield* request(9);
        yield* request(6, { document: (opened.value as Responses[2]).document });
        const closed = yield* request(9);
        assert.deepEqual(closed.value, { sourceBytes: 0, cacheBytes: 0, documents: 0, lists: 0 });
        cycles.push({
          cycle,
          document: doc.name,
          retained: retained.value,
          afterClose: closed.value,
          wasmBytes: closed.wasmBytes,
        });
      }
      results.memory!.push({
        mode,
        before: { ...before.value, wasmBytes: before.wasmBytes },
        cycles,
        growthBytes: cycles.at(-1)!.wasmBytes - before.wasmBytes,
        medianWasmBytes: median(cycles.map((c) => c.wasmBytes)),
      });
    }
}

function verify(doc: Doc, value: Tile | HostError, scale: number, x: number, y: number) {
  if (value instanceof Error) {
    assert.ok(doc.external && value instanceof HostError, String(value));
    assert.equal(value.category, 3, 'Only explicit Unsupported is a measured capability gap');
    return;
  }
  const size = doc.tileSize;
  assert.equal(value.width, size);
  assert.equal(value.height, size);
  if (scale === 1 && doc.rgb) {
    for (let row = 0; row < size; row++)
      for (let col = 0; col < size; col++) {
        const at = (row * size + col) * 4,
          expected = ((y + row) * doc.width! + x + col) * 3;
        for (let c = 0; c < 3; c++) assert.equal(value.rgba[at + c], doc.rgb[expected + c]);
        assert.equal(value.rgba[at + 3], 255);
      }
  }
  // Record byte identity for cross-runtime comparison, independently of timing.
  doc.outputs.push(hash(value.rgba));
}

async function nodeWasi(module: WebAssembly.Module, driver: Driver) {
  let instance: WebAssembly.Instance & { exports: { memory: WebAssembly.Memory } },
    current = new Uint8Array(),
    offset = 0,
    last: Reply | undefined,
    id = 0,
    start: number,
    waiting = false;
  const decoder = new FrameDecoder((frame) => {
    assert.equal(frame.id, id);
    last = { value: frame.value, ms: performance.now() - start, wasmBytes: instance.exports.memory.buffer.byteLength };
    waiting = false;
  });
  const wasi = new WASI({ version: 'preview1', args: [], env: {}, preopens: {}, returnOnExit: true });
  const imports = {
    ...wasi.wasiImport,
    fd_read(fd: number, iovs: number, count: number, result: number) {
      assert.equal(fd, 0);
      if (offset === current.length) {
        assert.equal(waiting, false, 'Host read before producing its previous response');
        const next = driver.next(last);
        if (next.done) {
          new DataView(instance.exports.memory.buffer).setUint32(result, 0, true);
          return 0;
        }
        start = performance.now();
        current = encodeRequest(next.value.kind, ++id, next.value.values);
        offset = 0;
        waiting = true;
      }
      const view = new DataView(instance.exports.memory.buffer);
      let total = 0;
      for (let i = 0; i < count && offset < current.length; i++) {
        const pointer = view.getUint32(iovs + i * 8, true),
          length = view.getUint32(iovs + i * 8 + 4, true);
        const n = Math.min(length, current.length - offset);
        new Uint8Array(instance.exports.memory.buffer, pointer, n).set(current.subarray(offset, offset + n));
        offset += n;
        total += n;
      }
      view.setUint32(result, total, true);
      return 0;
    },
    fd_write(fd: number, iovs: number, count: number, result: number) {
      const view = new DataView(instance.exports.memory.buffer);
      let total = 0;
      for (let i = 0; i < count; i++) {
        const pointer = view.getUint32(iovs + i * 8, true),
          length = view.getUint32(iovs + i * 8 + 4, true);
        const bytes = new Uint8Array(instance.exports.memory.buffer, pointer, length);
        if (fd === 1) decoder.feed(bytes);
        else process.stderr.write(bytes);
        total += length;
      }
      view.setUint32(result, total, true);
      return 0;
    },
  };
  instance = (await WebAssembly.instantiate(module, { wasi_snapshot_preview1: imports })) as typeof instance;
  assert.equal(wasi.start(instance), 0);
  decoder.finish();
}

async function shim(module: WebAssembly.Module, driver: Driver) {
  class Thread {
    listeners = new Map<string, Listener[]>();
    thread?: Worker;
    addEventListener(type: string, fn: Listener) {
      this.listeners.set(type, [...(this.listeners.get(type) ?? []), fn]);
    }
    emit(type: string, value: unknown) {
      for (const fn of this.listeners.get(type) ?? []) fn(value);
    }
    postMessage(message: { shared: ArrayBufferLike }) {
      this.thread = new Worker(new URL(import.meta.url), {
        execArgv: ['--disable-warning=ExperimentalWarning'],
        workerData: { module, shared: message.shared },
      });
      this.thread.on('message', (data) => this.emit('message', { data }));
      this.thread.on('error', (error) => this.emit('error', error));
    }
    terminate() {
      void this.thread?.terminate();
    }
  }
  const client = new SessionClient(new Thread());
  try {
    await client.ready;
    let previous: Reply | undefined;
    for (;;) {
      const next = driver.next(previous);
      if (next.done) break;
      const start = performance.now();
      let value: unknown;
      try {
        value = await client.request(next.value.kind, next.value.values);
      } catch (error) {
        if (!(error instanceof HostError)) throw error;
        value = error;
      }
      previous = { value, ms: performance.now() - start, wasmBytes: client.wasmBytes };
    }
  } finally {
    client.destroy();
  }
}

async function main() {
  const [wasmPath, externalPath] = process.argv.slice(2);
  if (!wasmPath || !externalPath) throw new Error('Expected WASM path and external multi-page PDF path');
  const wasm = readFileSync(wasmPath),
    module = await WebAssembly.compile(wasm),
    documents: Doc[] = [];
  for (const name of ['many-rectangles', 'two-pages', 'rectangle']) {
    const bytes = readFileSync(new URL(`../corpus/generated/classic/${name}.pdf`, import.meta.url));
    const ppm = readFileSync(new URL(`../corpus/generated/classic/${name}.ppm`, import.meta.url));
    const width = name === 'many-rectangles' ? 512 : 64;
    const rgb = ppm.subarray(Buffer.byteLength(`P6\n${width} ${width}\n255\n`));
    documents.push({ name, bytes, rgb, width, outputs: [], tileSize: width === 512 ? 256 : 32 });
  }
  documents.push({ name: externalPath, bytes: readFileSync(externalPath), external: true, outputs: [], tileSize: 128 });
  const report: Record<string, unknown> & { runtimes: object[] } = {
    schema: 1,
    exploratory: true,
    node: process.version,
    os: release(),
    cpu: cpus()[0]?.model,
    wasmSha256: hash(wasm),
    documents: documents.map((d) => ({ name: d.name, bytes: d.bytes.length, sha256: hash(d.bytes) })),
    runtimes: [],
  };
  const outputs: string[][][] = [];
  for (const [runtime, run] of [
    ['node-wasi', nodeWasi],
    ['browser-shim-node', shim],
  ] as const) {
    for (const d of documents) d.outputs = [];
    const result: Results & { runtime: string; fresh: object[] } = { runtime, fresh: [], latency: [], memory: [] };
    for (const doc of documents) {
      const samples: { firstMs: number; outcome: string }[] = [];
      for (let sample = -2; sample < 5; sample++) {
        const record: Results = { start: performance.now() };
        await run(module, workload([doc], record, 'fresh'));
        if (sample >= 0) samples.push({ firstMs: record.firstMs!, outcome: record.outcome! });
      }
      result.fresh.push({ document: doc.name, samples, medianMs: median(samples.map((s) => s.firstMs)) });
    }
    for (const phase of ['same', 'different', 'latency']) await run(module, workload(documents, result, phase));
    report.runtimes.push(result);
    outputs.push(documents.map((d) => d.outputs));
  }
  assert.deepEqual(outputs[0], outputs[1], 'All successful tile hashes must match across runtimes');
  report.pixelParity = true;
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
}

if (!isMainThread) {
  try {
    await runHost(workerData.module, workerData.shared, (message, transfer = []) =>
      parentPort!.postMessage(message, transfer),
    );
  } catch (error) {
    parentPort!.postMessage({ type: 'fatal', reason: failureReason(error), message: String(error) });
  }
} else await main();
