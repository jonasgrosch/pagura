/** Literal HTTP representations; no PDF parser or renderer is an oracle. */
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { gzipSync } from 'node:zlib';
import { httpSource } from '../web/source_http.mjs';
import { ProgressiveDriver } from '../web/progressive.mjs';

const url = 'https://source.invalid/document';
const bytes = Uint8Array.from([10, 20, 30, 40, 50, 60, 70, 80]);
const options = { maxBytes: 16, probeBytes: 4, rangeBytes: 8, spoolBytes: 16, transferBytes: 32 };
const extent = { start: 6n, end: 8n };
const tick = () => new Promise((resolve) => setImmediate(resolve));
const reply = (value, headers = {}, status = 206) => new Response(value, { status, headers });
const probe = () => reply(bytes.slice(0, 4), { etag: '"a"', 'content-range': 'bytes 0-3/8', 'content-length': '4' });
function script(responses) {
  const calls = [];
  return { calls, fetcher: async (address, init) => {
    calls.push({ address, ...init }); assert.ok(responses.length, 'unexpected HTTP retry');
    return responses.shift()(init);
  } };
}

async function sparse() {
  const transport = script([probe, () => reply(bytes.slice(6), { etag: '"a"', 'content-range': 'bytes 6-7/8', 'content-length': '2' })]);
  const source = await httpSource(url, { ...options, ...transport });
  try {
    assert.equal(source.mode, 'range'); assert.equal(source.length, 8n); assert.equal(source.version.length, 32);
    assert.deepEqual((await source.read({ start: 1n, end: 3n })).bytes, bytes.slice(1, 3));
    const packet = await source.read(extent);
    assert.deepEqual(packet.bytes, bytes.slice(6)); assert.equal(packet.offset, 6n); assert.equal(packet.length, 8n);
    assert.deepEqual(packet.version, source.version);
    assert.equal(transport.calls.length, 2);
    for (const call of transport.calls) {
      assert.equal(call.method, 'GET'); assert.equal(call.redirect, 'error'); assert.equal(call.credentials, 'omit');
      assert.equal(call.headers.has('accept-encoding'), false);
    }
    assert.equal(transport.calls[0].headers.get('range'), 'bytes=0-3');
    assert.equal(transport.calls[1].headers.get('range'), 'bytes=6-7');
    assert.equal(transport.calls[1].headers.get('if-range'), '"a"');
  } finally { source.close(); }
  await assert.rejects(source.read(extent), { name: 'AbortError' });
}

async function representationFailures() {
  for (const response of [
    () => reply(bytes.slice(6), { etag: '"b"', 'content-range': 'bytes 6-7/8' }),
    () => reply(bytes.slice(6), { etag: 'W/"a"', 'content-range': 'bytes 6-7/8' }),
    () => reply(bytes.slice(6), { 'content-range': 'bytes 6-7/8' }),
    () => reply(bytes.slice(6), { etag: '"a"', 'content-range': 'bytes 6-7/9' }),
    () => reply(bytes.slice(6), { etag: '"a"', 'content-range': 'bytes 5-6/8' }),
    () => reply(bytes.slice(6), { etag: '"a"', 'content-range': 'bytes 6-7/8', 'content-encoding': 'gzip' }),
    () => reply(bytes.slice(6), { etag: '"a"', 'content-range': 'bytes 6-7/8', 'content-length': '1' }),
    () => reply(bytes.slice(7), { etag: '"a"', 'content-range': 'bytes 6-7/8' }),
    () => reply(bytes.slice(5), { etag: '"a"', 'content-range': 'bytes 6-7/8' }),
    () => reply(bytes, { etag: '"a"' }, 200),
    () => reply(null, { 'content-range': 'bytes */8' }, 416),
    () => reply(null, {}, 503),
  ]) {
    const transport = script([probe, response]), source = await httpSource(url, { ...options, ...transport });
    await assert.rejects(source.read(extent)); assert.equal(source.signal.aborted, true);
    await assert.rejects(source.read({ start: 0n, end: 2n }));
    assert.equal(transport.calls.length, 2, 'terminal failures do not re-probe'); source.close();
  }
  const redirected = probe(); Object.defineProperty(redirected, 'url', { value: 'https://other.invalid/document' });
  await assert.rejects(httpSource(url, { ...options, ...script([() => redirected]) }), /endpoint/);
  await assert.rejects(httpSource(url, { ...options, ...script([() => reply(null, {}, 416)]) }), /probe/);
  for (const address of ['file:///tmp/file.pdf', 'https://name:secret@source.invalid/file', `${url}#part`]) await assert.rejects(httpSource(address), /URL/);
  await assert.rejects(httpSource(url, { headers: { 'accept-encoding': 'identity' } }), /controls/);
}

async function completeFallbacks() {
  for (const response of [
    () => reply(bytes.slice(0, 4), { etag: 'W/"a"', 'content-range': 'bytes 0-3/8' }),
    () => reply(bytes.slice(0, 4), { 'content-range': 'bytes 0-3/8' }),
    () => reply(bytes.slice(0, 4), { etag: '"a"', 'content-range': 'bytes 0-3/*' }),
    () => reply(bytes.slice(0, 4), { etag: '"a"', 'content-range': 'bytes 0-3/8', 'content-encoding': 'gzip' }),
    () => { const value = probe(); Object.defineProperty(value, 'type', { value: 'cors' }); return value; },
  ]) {
    const replacement = bytes.map((value) => value + 1), transport = script([response, () => reply(replacement, {}, 200)]);
    const source = await httpSource(url, { ...options, ...transport });
    try {
      assert.equal(source.mode, 'complete');
      assert.deepEqual((await source.read({ start: 0n, end: 8n })).bytes, replacement);
      assert.equal(transport.calls.length, 2); assert.equal(transport.calls[1].headers.has('range'), false);
      assert.equal(transport.calls[1].headers.has('if-range'), false);
    } finally { source.close(); }
  }
  const full = script([() => reply(bytes, { 'content-length': '8' }, 200)]);
  const complete = await httpSource(url, { ...options, ...full });
  assert.equal(complete.mode, 'complete'); assert.deepEqual((await complete.read(extent)).bytes, bytes.slice(6));
  assert.equal(full.calls.length, 1); complete.close();
  const visible = probe(); visible.headers.set('content-encoding', 'identity'); Object.defineProperty(visible, 'type', { value: 'cors' });
  const cors = await httpSource(url, { ...options, ...script([() => visible]) });
  assert.equal(cors.mode, 'range'); cors.close();
}

async function boundedBodiesAndDeadlines() {
  await assert.rejects(httpSource(url, { ...options, ...script([() => reply(new Uint8Array(17), {}, 200)]) }), /extent/);
  await assert.rejects(httpSource(url, { ...options, ...script([() => reply(bytes, { 'content-length': '17' }, 200)]) }), /extent/);
  await assert.rejects(httpSource(url, { ...options, ...script([() => reply(bytes, { 'content-length': '9' }, 200)]) }), /Truncated/);
  await assert.rejects(httpSource(url, { ...options, transferBytes: 3, ...script([probe]) }), /transfer budget/);
  const empty = new ReadableStream({ pull(controller) { controller.enqueue(new Uint8Array()); } });
  await assert.rejects(httpSource(url, { ...options, bodyChunks: 3, ...script([() => reply(empty, {}, 200)]) }), /body-read budget/);
  const big = new Uint8Array(65539).fill(7); let offset = 0;
  const chunks = new ReadableStream({ pull(controller) {
    if (offset === big.length) controller.close();
    else { controller.enqueue(big.slice(offset, offset + 3)); offset = Math.min(offset + 3, big.length); }
  } });
  const source = await httpSource(url, { maxBytes: big.length, rangeBytes: big.length, fetcher: async () => reply(chunks, {}, 200) });
  assert.equal(source.length, BigInt(big.length));
  assert.deepEqual((await source.read({ start: 65530n, end: 65539n })).bytes, new Uint8Array(9).fill(7)); source.close();
  const never = async (address, { signal }) => new Promise((resolve, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true }));
  await assert.rejects(httpSource(url, { ...options, fetcher: never, timeoutMs: 1 }), { name: 'TimeoutError' });
  const cancelled = new AbortController(); cancelled.abort(); let calls = 0;
  await assert.rejects(httpSource(url, { ...options, signal: cancelled.signal, fetcher: () => { calls++; } }), { name: 'AbortError' });
  assert.equal(calls, 0);
  const expiring = await httpSource(url, { ...options, ...script([probe]), idleMs: 1 });
  await new Promise((resolve) => expiring.signal.addEventListener('abort', resolve, { once: true }));
  await assert.rejects(expiring.read(extent), { name: 'TimeoutError' });
}

async function driverInvalidationAndAdmission() {
  const calls = [], client = {
    async createSource() { return { source: 1 }; }, async closeSource(source) { calls.push(['source-close', source]); },
    async advanceJob() { return { needData: true, source: 1n, version: source.version, generation: 0n, ranges: [extent] }; },
    async supplySource() { assert.fail('changed representation must not be supplied'); },
    async cancelJob(job) { calls.push(['cancel', job]); }, async closeJob(job) { calls.push(['job-close', job]); },
  };
  const source = await httpSource(url, { ...options, ...script([probe, () => reply(bytes.slice(6), { etag: '"b"', 'content-range': 'bytes 6-7/8' })]) });
  const driver = new ProgressiveDriver(client); await driver.register(source);
  await assert.rejects(driver.run(1).next(), /representation changed/); await driver.close();
  assert.deepEqual(calls, [['source-close', 1], ['cancel', 1], ['job-close', 1]]);
  const rejected = await httpSource(url, { ...options, ...script([probe]) });
  const failing = new ProgressiveDriver({ createSource: async () => { throw new Error('admission failed'); } });
  await assert.rejects(failing.register(rejected), /admission failed/);
  assert.equal(rejected.signal.aborted, true); await failing.close();
}

async function cancelledConsumerKeepsRepresentation() {
  const transport = script([probe,
    ({ signal }) => new Promise((resolve, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true })),
    () => reply(bytes.slice(6), { etag: '"a"', 'content-range': 'bytes 6-7/8' }),
  ]);
  const source = await httpSource(url, { ...options, ...transport });
  try {
    const controller = new AbortController();
    const cancelled = assert.rejects(source.read(extent, controller.signal), { name: 'AbortError' });
    await tick(); controller.abort(); await cancelled;
    assert.equal(source.signal.aborted, false);
    assert.deepEqual((await source.read(extent)).bytes, bytes.slice(6));
    assert.equal(transport.calls.length, 3);
  } finally { source.close(); }
  const calls = [], idle = await httpSource(url, { ...options, ...script([probe]), idleMs: 1 });
  const driver = new ProgressiveDriver({ async createSource() { return { source: 1 }; }, async closeSource(source) { calls.push(source); } });
  await driver.register(idle);
  await new Promise((resolve) => idle.signal.addEventListener('abort', resolve, { once: true }));
  await driver.close(); assert.deepEqual(calls, [1]);
}

async function actualFetch() {
  const requests = [], server = createServer((request, response) => {
    requests.push({ path: request.url, range: request.headers.range, conditional: request.headers['if-range'] });
    if (request.url === '/range') {
      const match = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? '');
      assert.ok(match); const start = Number(match[1]), end = Math.min(Number(match[2]) + 1, bytes.length);
      response.writeHead(206, { etag: '"fixed"', 'content-range': `bytes ${start}-${end - 1}/${bytes.length}`, 'content-length': end - start });
      response.end(bytes.slice(start, end));
    } else if (request.url === '/encoded') {
      const encoded = gzipSync(bytes); response.writeHead(200, { 'content-encoding': 'gzip', 'content-length': encoded.length }); response.end(encoded);
    } else if (request.url === '/truncated') {
      response.writeHead(200, { 'content-length': 9 }); response.end(bytes);
    } else if (request.url === '/redirect') { response.writeHead(302, { location: '/range' }); response.end(); }
    else { response.writeHead(416, { 'content-range': 'bytes */8' }); response.end(); }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const endpoint = `http://127.0.0.1:${server.address().port}`;
  try {
    const source = await httpSource(`${endpoint}/range`, options);
    try { assert.deepEqual((await source.read(extent)).bytes, bytes.slice(6)); } finally { source.close(); }
    const encoded = await httpSource(`${endpoint}/encoded`, options);
    try { assert.equal(encoded.mode, 'complete'); assert.deepEqual((await encoded.read({ start: 0n, end: 8n })).bytes, bytes); } finally { encoded.close(); }
    await assert.rejects(httpSource(`${endpoint}/truncated`, { ...options, timeoutMs: 20 }));
    await assert.rejects(httpSource(`${endpoint}/redirect`, options), /request failed/);
    await assert.rejects(httpSource(`${endpoint}/missing`, options), /probe/);
    assert.deepEqual(requests.filter((request) => request.path === '/range'), [
      { path: '/range', range: 'bytes=0-3', conditional: undefined },
      { path: '/range', range: 'bytes=6-7', conditional: '"fixed"' },
    ]);
  } finally { server.closeAllConnections(); await new Promise((resolve) => server.close(resolve)); }
}

await sparse();
await representationFailures();
await completeFallbacks();
await boundedBodiesAndDeadlines();
await driverInvalidationAndAdmission();
await cancelledConsumerKeepsRepresentation();
await actualFetch();
await tick();
console.log('Immutable HTTP source and driver lifecycle controls passed.');
