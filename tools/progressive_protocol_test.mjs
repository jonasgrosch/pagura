/** Independent V2 byte layouts and browser ownership controls. No PDF engine required. */
import assert from 'node:assert/strict';
import { decodeResponse, encodeRequest, HostError, responseMatches, VERSION_2 } from '../web/protocol.mjs';
import { SessionClient } from '../web/client.mjs';
import { ProgressiveDriver } from '../web/progressive.mjs';

const version = new Uint8Array(32).fill(27);
const u32 = (...values) => Buffer.concat(values.map((value) => { const bytes = Buffer.alloc(4); bytes.writeUInt32LE(value); return bytes; }));
const u64 = (...values) => Buffer.concat(values.map((value) => { const bytes = Buffer.alloc(8); bytes.writeBigUInt64LE(BigInt(value)); return bytes; }));
const body = (kind, payload) => Buffer.concat([Buffer.from([kind]), u32(3), payload]);
const tick = () => new Promise((resolve) => setImmediate(resolve));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };

function layouts() {
  const created = encodeRequest(41, 7, { length: 8192n, version });
  assert.deepEqual(Buffer.from(created), Buffer.concat([u32(45), Buffer.from([41]), u32(7), u64(8192), version]));
  const supply = encodeRequest(42, 7, { source: 4, version, offset: 1532n, bytes: Uint8Array.from([1, 0, 255]) });
  assert.deepEqual(Buffer.from(supply), Buffer.concat([u32(56), Buffer.from([42]), u32(7, 4), version, u64(1532), u32(3), Buffer.from([1, 0, 255])]));
  assert.deepEqual(Buffer.from(encodeRequest(49, 7, { job: 6, sequence: (1n << 64n) - 1n })), Buffer.concat([u32(17), Buffer.from([49]), u32(7, 6), Buffer.alloc(8, 255)]));
  for (const length of [-1, Number.MAX_SAFE_INTEGER + 1, 1n << 64n, '3']) assert.throws(() => encodeRequest(41, 1, { length, version }));
  assert.throws(() => encodeRequest(41, 1, { length: 0, version: new Uint8Array(31) }));
  const missing = body(179, Buffer.concat([u64(4), version, u64((1n << 64n) - 1n), u32(2), u64(100, 120, 140, 160)]));
  assert.deepEqual(decodeResponse(new Uint8Array(missing)).value, { needData: true, source: 4n, version, generation: (1n << 64n) - 1n, ranges: [{ start: 100n, end: 120n }, { start: 140n, end: 160n }] });
  const band = body(174, Buffer.concat([u32(6), Buffer.from([4]), u64(1), u32(0, 0, 1, 2, 0, 0, 1, 2, 0, 1, 4, 4), Buffer.from([0, 0, 0, 255])]));
  const decoded = decodeResponse(band).value;
  assert.equal(decoded.sequence, 1n); assert.equal(decoded.stride, 4); assert.deepEqual([...decoded.premultipliedRgba], [0, 0, 0, 255]);
  for (const input of [missing, band]) {
    for (let end = 0; end < input.length; end++) assert.throws(() => decodeResponse(input.subarray(0, end)));
    assert.throws(() => decodeResponse(Buffer.concat([input, Buffer.from([0])])));
  }
  for (const [offset, value] of [[57, 121n], [65, 100n], [73, 110n]]) {
    const invalid = Buffer.from(missing); invalid.writeBigUInt64LE(value, offset); assert.throws(() => decodeResponse(invalid));
  }
  const invalidBand = Buffer.from(band); invalidBand.writeUInt32LE(8, 58); assert.throws(() => decodeResponse(invalidBand));
  assert.equal(responseMatches(47, 173, VERSION_2), true);
  assert.equal(responseMatches(48, 179, VERSION_2), true);
  assert.equal(responseMatches(38, 179, VERSION_2), true);
  assert.equal(responseMatches(48, 176, VERSION_2), false);
  assert.equal(responseMatches(48, 179, 0x1000c), false);
  assert.equal(responseMatches(41, 179, VERSION_2), false);
}

async function serialClient() {
  const listeners = new Map(); let terminated = 0;
  const worker = { addEventListener(type, listener) { listeners.set(type, listener); }, postMessage() {}, terminate() { terminated++; } };
  const client = new SessionClient(worker);
  listeners.get('message')({ data: { type: 'ready' } });
  try {
    const hello = client.hello(VERSION_2); await tick();
    client.receive({ kind: 129, id: 1, value: { version: VERSION_2 } }); await hello;
    const started = client.renderJob(1, 0); await tick();
    client.receive({ kind: 173, id: 2, value: { job: 5 } }); assert.deepEqual(await started, { job: 5 });
    const waiting = client.advanceJob(5); await tick();
    client.receive({ kind: 179, id: 3, value: { needData: true } }); assert.equal((await waiting).needData, true);
    const failed = assert.rejects(client.renderJob(1, 0), (error) => error instanceof HostError && error.category === 2);
    await tick(); client.receive({ kind: 255, id: 4, value: new HostError(2, '', '', 'admission exceeded', {}) }); await failed;
    assert.equal(terminated, 0); assert.equal(client.failure, null);
    const cancelled = client.cancelJob(5); await tick();
    client.receive({ kind: 176, id: 5, value: { job: 5, changed: true } }); assert.equal((await cancelled).changed, true);
  } finally { client.destroy(); }
  assert.equal(terminated, 1);
}

function fakeClient(events) {
  const calls = []; let source = 0, generation = 0n;
  return { calls, async createSource() { return { source: ++source }; },
    async closeSource(source) { calls.push(['source-close', source]); },
    async supplySource(source, version, offset, bytes) { calls.push(['supply', source, offset, [...bytes]]); return { added: BigInt(bytes.length), generation: ++generation }; },
    async advanceJob(job) { calls.push(['advance', job]); const value = events.get(job)?.shift(); assert.ok(value, 'unexpected extra job step'); return value; },
    async acknowledgeJob(job, sequence) { calls.push(['ack', job, sequence]); },
    async cancelJob(job) { calls.push(['cancel', job]); }, async closeJob(job) { calls.push(['close', job]); } };
}
const demand = (start = 0n, end = 4n) => ({ needData: true, source: 1n, version, generation: 0n, ranges: [{ start, end }] });

async function sharedDemandAndOutput() {
  const input = deferred(), reads = [];
  const client = fakeClient(new Map([
    [1, [demand()]],
    [2, [demand(), { job: 2, event: 3 }, { job: 2, event: 5, sequence: 1n, bytes: Uint8Array.from([1, 2]) }, { job: 2, event: 6 }]],
  ]));
  const driver = new ProgressiveDriver(client, { concurrentRanges: 1, queuedRanges: 1 });
  await driver.register({ length: 4n, version, read(range, signal) { reads.push({ range, signal }); return input.promise; } });
  const abort = new AbortController(), first = driver.run(1, { signal: abort.signal }), second = driver.run(2);
  const left = first.next(), right = second.next(); await tick(); assert.equal(reads.length, 1);
  const cancelled = assert.rejects(left, { name: 'AbortError' }); abort.abort(); await cancelled;
  assert.equal(reads[0].signal.aborted, false);
  input.resolve({ length: 4n, version, offset: 0n, bytes: Uint8Array.from([4, 3, 2, 1]) });
  assert.equal((await right).value.event, 3); assert.equal(client.calls.filter(([kind]) => kind === 'supply').length, 1);
  assert.equal((await second.next()).value.event, 5);
  await tick(); assert.equal(client.calls.filter(([kind]) => kind === 'ack').length, 0);
  assert.equal((await second.next()).value.event, 6);
  assert.deepEqual(client.calls.filter(([kind]) => kind === 'ack'), [['ack', 2, 1n]]);
  assert.equal((await second.next()).done, true);
  assert.deepEqual(client.calls.filter(([kind]) => kind === 'cancel'), [['cancel', 1]]);
  assert.deepEqual(client.calls.filter(([kind]) => kind === 'close'), [['close', 1], ['close', 2]]);
  await driver.close();
}

async function cancellationAndShutdown() {
  const client = fakeClient(new Map([[1, [{ job: 1, event: 5, sequence: 1n, bytes: new Uint8Array(1) }]]]));
  const driver = new ProgressiveDriver(client), iterator = driver.run(1);
  await iterator.next(); await driver.close(); await iterator.return();
  assert.deepEqual(client.calls, [['advance', 1], ['cancel', 1], ['close', 1]]);
  const input = deferred(), registered = fakeClient(new Map()); registered.createSource = () => input.promise;
  const pending = new ProgressiveDriver(registered, { sources: 1 });
  const first = pending.register({ length: 4n, version, read() {} });
  await assert.rejects(pending.register({ length: 4n, version, read() {} }), /budget/);
  await pending.close(); input.resolve({ source: 1 });
  await assert.rejects(first, /closed/); assert.deepEqual(registered.calls, [['source-close', 1]]);
}

async function malformedRangesAndQueuedCancellation() {
  for (const change of [{ version: new Uint8Array(32) }, { length: 5n }, { offset: 1n }, { bytes: new Uint8Array(3) }]) {
    const client = fakeClient(new Map([[1, [demand()]]]));
    const driver = new ProgressiveDriver(client);
    await driver.register({ length: 4n, version, read: async () => ({ length: 4n, version, offset: 0n, bytes: new Uint8Array(4), ...change }) });
    await assert.rejects(driver.run(1).next(), /representation or extent/);
    assert.equal(client.calls.some(([kind]) => kind === 'supply'), false);
    await driver.close();
  }
  const reads = [], client = fakeClient(new Map([[1, [demand(0n, 2n)]], [2, [demand(2n, 4n)]], [3, [demand(4n, 6n)]]]));
  const driver = new ProgressiveDriver(client, { concurrentRanges: 1, queuedRanges: 1 });
  await driver.register({ length: 6n, version, read(range, signal) {
    reads.push(range); return new Promise((resolve, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true }));
  } });
  const abort = new AbortController(), first = driver.run(1), second = driver.run(2, { signal: abort.signal });
  const left = first.next(), right = second.next(); await tick(); assert.equal(reads.length, 1);
  await assert.rejects(driver.run(3).next(), /queue budget/);
  const cancellation = assert.rejects(right, { name: 'AbortError' }); abort.abort(); await cancellation;
  const shutdown = assert.rejects(left, { name: 'AbortError' }); await driver.close(); await shutdown;
  assert.equal(reads.length, 1);
}

async function cancelledRangeCanBeRequestedAgain() {
  const events = new Map([[1, [demand()]], [2, [demand(), { job: 2, event: 6 }]]]);
  const client = fakeClient(events), driver = new ProgressiveDriver(client); let reads = 0;
  await driver.register({ length: 4n, version, read(range, signal) {
    reads++;
    if (reads === 1) return new Promise((resolve, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true }));
    return Promise.resolve({ length: 4n, version, offset: range.start, bytes: new Uint8Array(4) });
  } });
  const controller = new AbortController(), abandoned = driver.run(1, { signal: controller.signal });
  const rejected = assert.rejects(abandoned.next(), { name: 'AbortError' });
  await tick(); controller.abort(); await rejected;
  const retry = driver.run(2); assert.equal((await retry.next()).value.event, 6); await retry.next();
  assert.equal(reads, 2); await driver.close();
  const stalled = fakeClient(new Map([[1, [demand()]]]));
  stalled.supplySource = async () => ({ added: 0n, generation: 0n });
  const noProgress = new ProgressiveDriver(stalled);
  await noProgress.register({ length: 4n, version, read: async () => ({ length: 4n, version, offset: 0n, bytes: new Uint8Array(4) }) });
  await assert.rejects(noProgress.run(1).next(), /no availability progress/); await noProgress.close();
}

async function ignoredProviderCancellation() {
  const client = fakeClient(new Map([[1, [demand()]], [2, [demand()]]]));
  const driver = new ProgressiveDriver(client, { concurrentRanges: 1 });
  let reads = 0;
  await driver.register({ length: 4n, version, read() { reads++; return new Promise(() => {}); } });
  const left = driver.run(1), right = driver.run(2);
  const stopped = Promise.all([assert.rejects(left.next(), { name: 'AbortError' }), assert.rejects(right.next(), { name: 'AbortError' })]);
  await tick(); assert.equal(reads, 1);
  const stale = driver.sources.get(1);
  await driver.closeSource(1);
  // A deterministic microtask deadline prevents a broken waiter from hanging CI.
  assert.equal(await Promise.race([stopped.then(() => true), tick().then(() => false)]), true);
  assert.equal(driver.io.active, 1, 'ignored provider I/O remains charged');
  assert.equal(driver.jobs.size, 0);
  await assert.rejects(driver.range(1, stale, 0n, 4n), { name: 'AbortError' });
  assert.equal(reads, 1);
  await driver.close();

  const pendingClient = fakeClient(new Map([[1, [demand()]]]));
  const pending = new ProgressiveDriver(pendingClient);
  await pending.register({ length: 4n, version, read: () => new Promise(() => {}) });
  const ended = assert.rejects(pending.run(1).next(), { name: 'AbortError' });
  await tick(); await pending.close();
  assert.equal(await Promise.race([ended.then(() => true), tick().then(() => false)]), true);
  assert.equal(pending.io.active, 1);
}

layouts();
await serialClient();
await sharedDemandAndOutput();
await cancellationAndShutdown();
await malformedRangesAndQueuedCancellation();
await cancelledRangeCanBeRequestedAgain();
await ignoredProviderCancellation();
console.log('Progressive protocol and browser ownership controls passed.');
