import { JobEventCode, MAX_FILE } from './protocol_constants.mjs';

function cancelled(signal) { return signal?.reason ?? new DOMException('Operation cancelled', 'AbortError'); }
function check(signal) { if (signal?.aborted) throw cancelled(signal); }
function sameVersion(left, right) { return right instanceof Uint8Array && right.length === 32 && left.every((byte, index) => byte === right[index]); }
function exact(value) {
  if (typeof value === 'number' && !Number.isSafeInteger(value)) throw new Error('Inexact source length or offset');
  if (typeof value !== 'number' && typeof value !== 'bigint') throw new Error('Invalid source length or offset');
  return BigInt(value);
}
function wait(promise, ...signals) {
  signals = signals.filter(Boolean);
  for (const signal of signals) check(signal);
  return new Promise((resolve, reject) => {
    const listeners = signals.map((signal) => [signal, () => finish(reject, cancelled(signal))]);
    function finish(complete, value) {
      for (const [signal, listener] of listeners) signal.removeEventListener('abort', listener);
      complete(value);
    }
    for (const [signal, listener] of listeners) signal.addEventListener('abort', listener, { once: true });
    promise.then((value) => finish(resolve, value), (error) => finish(reject, error));
  });
}

/** Bounds asynchronous range operations; no CPU worker or engine lock waits here. */
class Ranges {
  constructor(active, queued) { this.limit = active; this.capacity = queued; this.active = 0; this.queue = []; }
  run(work, signal) {
    check(signal);
    if (this.queue.length >= this.capacity && this.active >= this.limit) return Promise.reject(new Error('Range queue budget exceeded'));
    return new Promise((resolve, reject) => {
      const task = { work, signal, resolve, reject };
      task.abort = () => { this.queue = this.queue.filter((entry) => entry !== task); reject(cancelled(signal)); };
      signal.addEventListener('abort', task.abort, { once: true });
      this.queue.push(task); this.pump();
    });
  }
  pump() {
    while (this.active < this.limit && this.queue.length) {
      const task = this.queue.shift(); task.signal.removeEventListener('abort', task.abort);
      if (task.signal.aborted) { task.reject(cancelled(task.signal)); continue; }
      this.active++;
      Promise.resolve().then(task.work).then(task.resolve, task.reject).finally(() => { this.active--; this.pump(); });
    }
  }
}

/** Drives the existing V2 client from caller-verified immutable ranges.
 * `read({start,end}, signal)` returns `{version,length,offset,bytes}`. The caller
 * owns URL/validator/security policy; no source-controlled address is followed.
 * The engine sees only exact, bounded bytes. HTTP/native/browser worker-pool
 * adapters remain separate consumers of this contract.
 */
export class ProgressiveDriver {
  constructor(client, { rangeBytes = 1024 * 1024, concurrentRanges = 4, queuedRanges = 32, sources = 16 } = {}) {
    for (const value of [rangeBytes, concurrentRanges, queuedRanges, sources]) if (!Number.isSafeInteger(value) || value <= 0) throw new Error('Invalid progressive driver quota');
    if (rangeBytes > MAX_FILE) throw new Error('Range supply exceeds the frame budget');
    this.client = client; this.rangeBytes = rangeBytes; this.sourceLimit = sources;
    this.io = new Ranges(concurrentRanges, queuedRanges); this.sources = new Map(); this.jobs = new Set(); this.registrations = 0; this.closed = false;
  }
  async register({ length, version, read }) {
    length = exact(length);
    if (this.closed || length < 0n || length > BigInt(MAX_FILE) || !(version instanceof Uint8Array) || version.length !== 32 || typeof read !== 'function') throw new Error('Invalid immutable source registration');
    if (this.sources.size + this.registrations >= this.sourceLimit) throw new Error('Source registry budget exceeded');
    version = version.slice(); this.registrations++;
    try {
      const { source } = await this.client.createSource(length, version);
      if (this.closed) { await this.client.closeSource(source); throw new Error('Progressive driver closed'); }
      this.sources.set(source, { length, version, read, pending: new Map(), closed: false });
      return source;
    } finally { this.registrations--; }
  }
  async closeSource(handle) {
    const source = this.sources.get(handle);
    if (!source) throw new Error('Unknown driver source');
    source.closed = true; this.sources.delete(handle);
    for (const entry of source.pending.values()) entry.controller.abort();
    await this.client.closeSource(handle);
  }
  async fill(demand, signal) {
    const handle = Number(demand.source), source = this.sources.get(handle);
    if (!Number.isSafeInteger(handle) || !source || source.closed || !sameVersion(source.version, demand.version)) throw new Error('Demand belongs to another source representation');
    let generation = demand.generation;
    for (const range of demand.ranges) {
      if (range.start < 0n || range.end > source.length || range.start >= range.end) throw new Error('Demand exceeds source bounds');
      for (let start = range.start; start < range.end;) {
        check(signal);
        const end = start + BigInt(this.rangeBytes) < range.end ? start + BigInt(this.rangeBytes) : range.end;
        const supplied = await this.range(handle, source, start, end, signal);
        if (supplied.generation > generation) generation = supplied.generation;
        start = end;
      }
    }
    if (generation <= demand.generation) throw new Error('Range supply made no availability progress');
  }
  async range(handle, source, start, end, signal) {
    check(signal);
    if (this.closed || source.closed) throw new DOMException('Source closed', 'AbortError');
    const key = `${start}:${end}`;
    let entry = source.pending.get(key);
    if (!entry) {
      entry = { controller: new AbortController(), waiters: 0, done: false };
      const current = entry;
      current.promise = this.io.run(async () => {
        check(current.controller.signal);
        if (this.closed || source.closed) throw new DOMException('Source closed', 'AbortError');
        const packet = await source.read({ start, end }, current.controller.signal);
        check(current.controller.signal);
        if (source.closed || !sameVersion(source.version, packet.version) || exact(packet.length) !== source.length
          || exact(packet.offset) !== start || !(packet.bytes instanceof Uint8Array) || BigInt(packet.bytes.length) !== end - start) throw new Error('Range response changed representation or extent');
        // Freeze the verified packet while it waits in the shared serial queue.
        // At most one received packet plus one supplied copy is held per range.
        return this.client.supplySource(handle, source.version, start, packet.bytes.slice());
      }, current.controller.signal).finally(() => {
        current.done = true;
        if (source.pending.get(key) === current) source.pending.delete(key);
      });
      // Last-consumer cancellation may precede settlement of an external read.
      current.promise.catch(() => {});
      source.pending.set(key, current);
    }
    entry.waiters++;
    // Closing shared demand ends every waiter even if an external provider
    // ignores cancellation. Its actual I/O slot stays occupied until it settles.
    try { return await wait(entry.promise, signal, entry.controller.signal); }
    finally {
      entry.waiters--;
      if (!entry.waiters && !entry.done) {
        entry.controller.abort();
        if (source.pending.get(key) === entry) source.pending.delete(key);
      }
    }
  }
  /** Each yielded output remains unacknowledged until the consumer asks again.
   * Breaking iteration cancels and closes this job; shared source demand survives
   * while another job still owns it. `current` can reject an obsolete viewport.
   */
  async *run(job, { signal, current = () => true } = {}) {
    if (this.closed || this.jobs.has(job)) throw new Error('Job already driven or driver closed');
    this.jobs.add(job); let complete = false, failure = null, sequence = 0n;
    try {
      while (!complete) {
        check(signal);
        if (this.closed || !current()) throw new DOMException('Obsolete job', 'AbortError');
        const result = await this.client.advanceJob(job);
        check(signal);
        if (this.closed) throw new DOMException('Progressive driver closed', 'AbortError');
        if (result.needData) { await this.fill(result, signal); continue; }
        if (result.job !== job) throw new Error('Progress belongs to another job');
        if (result.event === JobEventCode.Yielded) continue;
        if (result.event === JobEventCode.Backpressured) throw new Error('Job has output owned by another consumer');
        if (result.event === JobEventCode.Cancelled) throw new DOMException('Job cancelled', 'AbortError');
        if (!current()) throw new DOMException('Obsolete job', 'AbortError');
        complete = result.event === JobEventCode.Complete;
        const output = result.event === JobEventCode.Band || result.event === JobEventCode.Png;
        if (output && result.sequence !== ++sequence) throw new Error('Nonconsecutive output sequence');
        yield result;
        if (output) {
          check(signal);
          if (this.closed || !current()) throw new DOMException('Obsolete job', 'AbortError');
          await this.client.acknowledgeJob(job, sequence);
        }
      }
    } catch (error) { failure = error; throw error; }
    finally {
      if (this.jobs.delete(job)) {
        const errors = [];
        if (!complete) { try { await this.client.cancelJob(job); } catch (error) { errors.push(error); } }
        try { await this.client.closeJob(job); } catch (error) { errors.push(error); }
        if (!failure && errors.length) throw new AggregateError(errors, 'Job cleanup failed');
      }
    }
  }
  async close() {
    this.closed = true;
    const results = await Promise.allSettled([...this.sources.keys()].map((source) => this.closeSource(source)));
    for (const job of [...this.jobs]) {
      if (!this.jobs.delete(job)) continue;
      try { await this.client.cancelJob(job); } catch (reason) { results.push({ status: 'rejected', reason }); }
      try { await this.client.closeJob(job); } catch (reason) { results.push({ status: 'rejected', reason }); }
    }
    const errors = results.filter((result) => result.status === 'rejected').map((result) => result.reason);
    if (errors.length) throw new AggregateError(errors, 'Progressive driver cleanup failed');
  }
}
