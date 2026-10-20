import { HostFailure } from './failure.mjs';
import { encodeRequest, FrameDecoder, HostError, VERSION_1_4, VERSION_1_5 } from './protocol.mjs';
import { createInput, InputWriter } from './transport.mjs';

/** Long-lived serial session. Recreate the client after a trap/terminal failure. */
export class SessionClient {
  constructor(worker, { onFailure = () => {}, timeoutMs = 30000, debug = false } = {}) {
    this.onFailure = onFailure; this.timeoutMs = timeoutMs; this.debug = debug; this.wasmBytes = 0;
    this.timer = setTimeout(() => this.fail(new HostFailure('deadline exceeded', 'startup exceeded its time budget')), timeoutMs);
    this.worker = worker; this.input = new InputWriter(createInput()); this.nextId = 1;
    this.searchEpoch = 0; this.generations = new Map(); this.selectedGenerations = new Map();
    this.tail = Promise.resolve(); this.pending = null; this.failure = null; this.queued = 0;
    this.decoder = new FrameDecoder((frame) => this.receive(frame));
    this.ready = new Promise((resolve, reject) => { this.resolveReady = resolve; this.rejectReady = reject; });
    // Keep worker startup failures observed even before the first request.
    this.ready.catch(() => {});
    worker.addEventListener('message', ({ data }) => {
      if (this.failure) return;
      try {
        if (data.type === 'ready') { clearTimeout(this.timer); this.wasmBytes = data.wasmBytes ?? 0; this.resolveReady(); }
        else if (data.type === 'memory') this.wasmBytes = data.wasmBytes;
        else if (data.type === 'drain') this.input.pump();
        else if (data.type === 'stdout') this.decoder.feed(data.bytes);
        else if (data.type === 'fatal') this.fail(new HostFailure(data.reason ?? 'execution failure', data.message));
        else if (data.type === 'exit') { this.decoder.finish(); this.fail(new Error(`Host session exited (${data.code})`)); }
      } catch (error) { this.fail(error); }
    });
    worker.addEventListener('error', (event) => this.fail(new HostFailure('execution failure', event.message || 'Host execution failed')));
    worker.addEventListener('messageerror', () => this.fail(new Error('Invalid worker message')));
    worker.postMessage({ type: 'start', shared: this.input.control.buffer });
  }
  receive(frame) {
    if (frame.kind === 255 && frame.id === 0) { this.fail(frame.value); return; }
    const pending = this.pending;
    if (!pending || frame.id !== pending.id || (frame.kind !== 255 && frame.kind !== pending.kind + 128)) throw new Error('Mismatched host response');
    if (frame.value instanceof HostError && frame.value.category === 2) {
      this.fail(new HostFailure('budget exceeded', frame.value)); return;
    }
    clearTimeout(this.timer); this.pending = null;
    if (frame.value instanceof HostError) pending.reject(frame.value); else pending.resolve(frame.value);
  }
  fail(error, notify = true) {
    if (this.failure) return;
    clearTimeout(this.timer);
    this.failure = error; this.rejectReady(error);
    if (this.pending) { this.pending.reject(error); this.pending = null; }
    this.input.queue = []; this.input.offset = 0;
    this.decoder = new FrameDecoder(() => {}); this.worker.terminate();
    if (notify) queueMicrotask(() => this.onFailure(error));
  }
  request(kind, values) {
    if (this.failure) return Promise.reject(this.failure);
    if (this.queued >= 64) return Promise.reject(new Error('Too many queued host requests'));
    this.queued++;
    const result = this.tail.then(async () => {
      await this.ready;
      if (this.failure) throw this.failure;
      if (kind === 11 && values.token !== undefined && values.token !== this.searchEpoch) return { cancelled: true };
      if ((kind === 7 || kind === 8) && this.generations.get(values.document) !== values.generation) {
        return { cancelled: true, generation: values.generation };
      }
      if (this.nextId > 0xffffffff) throw new Error('Request identifiers exhausted; restart the session');
      const id = this.nextId++, bytes = encodeRequest(kind, id, values);
      return new Promise((resolve, reject) => {
        this.pending = { id, kind, resolve, reject };
        this.timer = setTimeout(() => this.fail(new HostFailure('deadline exceeded', 'request exceeded its time budget')), this.timeoutMs);
        try { this.input.write(bytes); } catch (error) { this.fail(error); }
      });
    });
    this.tail = result.then(() => {}, () => {}).finally(() => { this.queued--; });
    return result;
  }
  async hello(version = VERSION_1_5) {
    const result = await this.request(1, { version });
    if (result.version !== version) { const error = new Error('Unsupported host protocol version'); this.fail(error); throw error; }
    this.version = version;
    return result;
  }
  open(bytes, recovery = 1, password = new Uint8Array()) { return this.request(2, { bytes, recovery, password }); }
  formFields(document) { return this.request(18, { document }); }
  saveForm(document, changes) { return this.request(19, { document, changes }); }
  pageInfo(document, page) { return this.request(3, { document, page }); }
  contentBounds(document, page) { return this.request(16, { document, page }); }
  renderTile(document, page, scale, tile) { return this.request(4, { document, page, scale, ...tile }); }
  invalidate(document) {
    const generation = (this.generations.get(document) ?? 0) + 1;
    if (generation > 0xffffffff) throw new Error('Viewport generations exhausted; reopen the document');
    this.generations.set(document, generation);
    return generation;
  }
  async renderGenerationTile(document, page, scale, tile, generation) {
    if (this.generations.get(document) !== generation) return { cancelled: true, generation };
    if (this.selectedGenerations.get(document) !== generation) {
      this.selectedGenerations.set(document, generation);
      await this.request(7, { document, generation });
    }
    return this.request(8, { document, page, scale, ...tile, generation });
  }
  pageText(document, page) { return this.request(10, { document, page }); }
  invalidateSearch() { return ++this.searchEpoch; }
  search(document, query, start, end, offset = 0, limit = 128, flags = 4, token = this.searchEpoch) { return this.request(11, { document, query, start, end, offset, limit, flags, token }); }
  links(document, page) { return this.request(12, { document, page }); }
  outlines(document) { return this.request(13, { document }); }
  labels(document) { return this.request(14, { document }); }
  readingStats() { return this.request(15); }
  stats() { return this.request(9); }
  /** Development injection stays in the client; no engine instruction changes. */
  injectFailure(reason) {
    if (!this.debug) throw new Error('Failure injection is disabled');
    const failures = { trap: new WebAssembly.RuntimeError('injected trap'), memory: new RangeError('injected memory exhaustion'),
      budget: new HostError(2, '', '', 'injected budget exhaustion', {}), deadline: new Error('injected deadline') };
    if (!Object.hasOwn(failures, reason)) throw new Error('Unknown injected failure');
    const names = { trap: 'trap', memory: 'memory failure', budget: 'budget exceeded', deadline: 'deadline exceeded' };
    this.fail(new HostFailure(names[reason], failures[reason]));
  }
  diagnostics(document) { return this.request(this.version >= VERSION_1_4 ? 17 : 5, { document }); }
  close(document) {
    this.generations.delete(document); this.selectedGenerations.delete(document);
    return this.request(6, { document });
  }
  destroy() { this.fail(new Error('Host session closed'), false); }
}
