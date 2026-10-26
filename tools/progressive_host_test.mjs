/** Hosted native/WASI/browser source-to-band/PNG acceptance using a literal PDF. */
import assert from 'node:assert/strict';
import { readFile, mkdtemp, open, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { inflateSync } from 'node:zlib';
import { WASI } from 'node:wasi';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';
import { SessionClient } from '../web/client.mjs';
import { encodeRequest, FrameDecoder, VERSION_2 } from '../web/protocol.mjs';
import { ProgressiveDriver } from '../web/progressive.mjs';
import { runHost } from '../web/worker.mjs';

const version = new Uint8Array(32).fill(27);

if (!isMainThread) {
  try { await runHost(workerData.module, workerData.shared, (message, transfer = []) => parentPort.postMessage(message, transfer)); }
  catch (error) { parentPort.postMessage({ type: 'fatal', message: String(error) }); }
} else {
  await main();
}

function literal() {
  const pdf = new Uint8Array(8192).fill(32), text = new TextEncoder();
  for (const [offset, value] of [
    [0, '%PDF-1.7\n'], [100, '1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n'],
    [600, '2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 8 8] >> endobj\n'],
    [900, '3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n'],
    [1500, '4 0 obj << /Length 13 >> stream\n0 0 8 8 re f\n\nendstream\nendobj\n'],
    [4000, '5 0 obj null endobj\n'],
    [6500, 'xref\n0 6\n0 65535 f\n100 0 n\n600 0 n\n900 0 n\n1500 0 n\n4000 0 n\ntrailer << /Size 6 /Root 1 0 R >>\n'],
    [8100, 'startxref\n6500\n%%EOF\n'],
  ]) pdf.set(text.encode(value), offset);
  return pdf;
}
function frames(bytes) { const values = [], decoder = new FrameDecoder((frame) => values.push(frame)); decoder.feed(bytes); decoder.finish(); return values; }
function requests(pdf) {
  const input = []; let id = 0;
  const add = (kind, values = {}) => input.push(encodeRequest(kind, ++id, values));
  const advance = (job) => { for (let count = 0; count < 64; count++) add(48, { job }); };
  const supply = (start, end) => add(42, { source: 1, version, offset: BigInt(start), bytes: pdf.subarray(start, end) });
  add(1, { version: VERSION_2 }); add(41, { length: 8192n, version });
  add(45, { source: 1, validation: 1, recovery: 0, password: new Uint8Array() });
  add(48, { job: 1 }); // Missing header must release the serial request queue.
  supply(0, 1024); supply(6500, 6800); supply(7168, 8192); advance(1);
  add(51, { job: 1 });
  add(47, { document: 1, page: 0, scale: 1, background: 0, output: 0 }); advance(2);
  supply(1500, 1532); supply(1545, 1564); advance(2);
  supply(1532, 1545); advance(2);
  add(49, { job: 2, sequence: 1n }); add(48, { job: 2 }); add(51, { job: 2 });
  add(47, { document: 1, page: 0, scale: 1, background: 0, output: 1 });
  for (let sequence = 1n; sequence <= 3n; sequence++) { advance(3); add(49, { job: 3, sequence }); }
  add(48, { job: 3 }); add(51, { job: 3 }); add(6, { document: 1 }); add(44, { source: 1 }); add(52);
  return Buffer.concat(input);
}

async function wasiRun(module, input, folder, label) {
  const source = join(folder, `${label}.input`), destination = join(folder, `${label}.output`), errors = join(folder, `${label}.error`);
  const writer = await open(source, 'w'); await writer.writeFile(input); await writer.close();
  const handles = await Promise.all([open(source, 'r'), open(destination, 'w'), open(errors, 'w')]);
  try {
    const wasi = new WASI({ version: 'preview1', args: [], env: {}, preopens: {}, returnOnExit: true,
      stdin: handles[0].fd, stdout: handles[1].fd, stderr: handles[2].fd });
    const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
    assert.equal(wasi.start(instance), 0, (await readFile(errors)).toString());
    return readFile(destination);
  } finally { await Promise.all(handles.map((handle) => handle.close())); }
}

async function browser(module, pdf) {
  class TestWorker {
    constructor() { this.listeners = new Map(); }
    addEventListener(type, listener) { const listeners = this.listeners.get(type) ?? []; listeners.push(listener); this.listeners.set(type, listeners); }
    postMessage(message) {
      this.worker = new Worker(new URL(import.meta.url), { execArgv: ['--disable-warning=ExperimentalWarning'], workerData: { module, shared: message.shared } });
      this.worker.on('message', (data) => this.emit('message', { data })); this.worker.on('error', (error) => this.emit('error', error));
    }
    emit(type, value) { for (const listener of this.listeners.get(type) ?? []) listener(value); }
    terminate() { void this.worker?.terminate(); }
  }
  const client = new SessionClient(new TestWorker()), driver = new ProgressiveDriver(client), reads = [];
  try {
    await client.hello(VERSION_2);
    const source = await driver.register({ length: 8192n, version, async read(range) {
      assert.ok(range.end <= 4000n || range.start >= 5000n, 'unrelated object must remain unfetched');
      reads.push(range); return { version, length: 8192n, offset: range.start, bytes: pdf.slice(Number(range.start), Number(range.end)) };
    } });
    let document;
    const opening = await client.openJob(source, { validation: 1, recovery: 0 });
    for await (const value of driver.run(opening.job)) if (value.event === 1) document = value.document;
    assert.ok(document);
    const page = await client.pageJob(document, 0); let pageInfo;
    for await (const value of driver.run(page.job)) if (value.event === 2) pageInfo = value;
    assert.deepEqual(pageInfo.crop, [0, 0, 8, 8]); assert.equal(pageInfo.verifiedCount, null);
    const rendered = await client.renderJob(document, 0, { output: 0 }); let bands = 0;
    for await (const value of driver.run(rendered.job)) if (value.event === 4) {
      bands++; assert.deepEqual([...value.premultipliedRgba], Array.from({ length: 256 }, (_, index) => index % 4 === 3 ? 255 : 0));
      assert.equal((await client.advanceJob(rendered.job)).event, 8, 'held consumer output must backpressure the canonical producer');
    }
    assert.equal(bands, 1);
    const encoded = await client.renderJob(document, 0, { output: 1 }), chunks = [];
    for await (const value of driver.run(encoded.job)) if (value.event === 5) chunks.push(value.bytes);
    checkPng(Buffer.concat(chunks));
    // Aggregate source admission refuses a fifth source without killing V2.
    const extras = [];
    for (let count = 0; count < 3; count++) extras.push((await client.createSource(8n, version)).source);
    await assert.rejects(client.createSource(8n, version), (error) => error.category === 2);
    assert.equal(client.failure, null); for (const handle of extras) await client.closeSource(handle);
    await client.close(document); await driver.closeSource(source);
    assert.deepEqual(await client.jobStats(), { sources: 0, jobs: 0, sourceAdmission: 0n, activeAdmission: 0n, sourceResidency: 0n });
    const received = reads.reduce((sum, range) => sum + Number(range.end - range.start), 0);
    assert.ok(received < 8192, 'source driver must not materialize the whole input');
    return { requests: reads.length, received };
  } finally { await driver.close(); client.destroy(); }
}

function crc(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes) { value ^= byte; for (let bit = 0; bit < 8; bit++) value = value & 1 ? (value >>> 1) ^ 0xedb88320 : value >>> 1; }
  return (value ^ 0xffffffff) >>> 0;
}
function checkPng(bytes) {
  assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
  const compressed = [], types = []; let offset = 8;
  while (offset < bytes.length) {
    const length = bytes.readUInt32BE(offset), type = bytes.toString('ascii', offset + 4, offset + 8), end = offset + 12 + length;
    assert.ok(end <= bytes.length); assert.equal(bytes.readUInt32BE(end - 4), crc(bytes.subarray(offset + 4, end - 4)));
    types.push(type);
    if (type === 'IHDR') assert.deepEqual([...bytes.subarray(offset + 8, end - 4)], [0, 0, 0, 8, 0, 0, 0, 8, 8, 6, 0, 0, 0]);
    else if (type === 'IDAT') compressed.push(bytes.subarray(offset + 8, end - 4));
    else { assert.equal(type, 'IEND'); assert.equal(length, 0); assert.equal(end, bytes.length); }
    offset = end;
  }
  assert.deepEqual(types, ['IHDR', 'IDAT', 'IEND']);
  const filtered = inflateSync(Buffer.concat(compressed), { maxOutputLength: 264 }); assert.equal(filtered.length, 264);
  const pixels = Buffer.alloc(256);
  for (let row = 0; row < 8; row++) {
    const filter = filtered[row * 33]; assert.ok(filter <= 4);
    for (let column = 0; column < 32; column++) {
      const at = row * 32 + column, left = column >= 4 ? pixels[at - 4] : 0, up = row ? pixels[at - 32] : 0, corner = row && column >= 4 ? pixels[at - 36] : 0;
      const prediction = left + up - corner, distances = [Math.abs(prediction - left), Math.abs(prediction - up), Math.abs(prediction - corner)];
      const paeth = distances[0] <= distances[1] && distances[0] <= distances[2] ? left : distances[1] <= distances[2] ? up : corner;
      pixels[at] = (filtered[row * 33 + column + 1] + [0, left, up, Math.floor((left + up) / 2), paeth][filter]) & 255;
    }
  }
  assert.deepEqual([...pixels], Array.from({ length: 256 }, (_, index) => index % 4 === 3 ? 255 : 0));
}

async function main() {
  const pdf = literal(), input = requests(pdf), native = spawnSync('target/debug/pagura-host', [], { input, timeout: 30000, maxBuffer: 8 * 1024 * 1024 });
  assert.ifError(native.error); assert.equal(native.status, 0, native.stderr?.toString());
  const output = frames(native.stdout); assert.ok(output.every((frame) => frame.kind !== 255));
  assert.ok(output.some(({ kind, value }) => kind === 179 && value.ranges.some(({ start, end }) => start === 1532n && end === 1545n)));
  const bands = output.filter(({ kind, value }) => kind === 174 && value.event === 4);
  assert.equal(bands.length, 1); assert.deepEqual([...bands[0].value.premultipliedRgba], Array.from({ length: 256 }, (_, index) => index % 4 === 3 ? 255 : 0));
  checkPng(Buffer.concat(output.filter(({ kind, value }) => kind === 174 && value.event === 5).map(({ value }) => value.bytes)));
  assert.deepEqual(output.at(-1).value, { sources: 0, jobs: 0, sourceAdmission: 0n, activeAdmission: 0n, sourceResidency: 0n });
  const paths = process.argv.slice(2); if (!paths.length) paths.push('target/wasm32-wasip1/release/pagura-host.wasm');
  const scratch = await mkdtemp(join(tmpdir(), 'pagura-progressive-'));
  try {
    for (const [index, path] of paths.entries()) {
      const module = await WebAssembly.compile(await readFile(path));
      assert.deepEqual(await wasiRun(module, input, scratch, `module-${index}`), native.stdout, 'native and WASI wire/output bytes');
      const observed = await browser(module, pdf); console.log(JSON.stringify({ module: path, ...observed }));
    }
  } finally { await rm(scratch, { recursive: true, force: true }); }
  console.log('Progressive native/WASI/browser output matched independent literal pixels and PNG decoding.');
}
