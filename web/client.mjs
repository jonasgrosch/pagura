import { encodeRequest, FrameDecoder, HostError } from './protocol.mjs';
import { createInput, InputWriter } from './transport.mjs';

/** Long-lived serial session. Recreate the client after a trap/terminal failure. */
export class SessionClient {
  constructor(worker) {
    this.worker = worker; this.input = new InputWriter(createInput()); this.nextId = 1;
    this.tail = Promise.resolve(); this.pending = null; this.failure = null; this.queued = 0;
    this.decoder = new FrameDecoder((frame) => this.receive(frame));
    this.ready = new Promise((resolve, reject) => { this.resolveReady = resolve; this.rejectReady = reject; });
    // Keep worker startup failures observed even before the first request.
    this.ready.catch(() => {});
    worker.addEventListener('message', ({ data }) => {
      try {
        if (data.type === 'ready') this.resolveReady();
        else if (data.type === 'drain') this.input.pump();
        else if (data.type === 'stdout') this.decoder.feed(data.bytes);
        else if (data.type === 'fatal') this.fail(new Error(data.message));
        else if (data.type === 'exit') { this.decoder.finish(); this.fail(new Error(`Host session exited (${data.code})`)); }
      } catch (error) { this.fail(error); }
    });
    worker.addEventListener('error', (event) => this.fail(new Error(event.message || 'Host worker failed')));
    worker.addEventListener('messageerror', () => this.fail(new Error('Invalid worker message')));
    worker.postMessage({ type: 'start', shared: this.input.control.buffer });
  }
  receive(frame) {
    if (frame.kind === 255 && frame.id === 0) { this.fail(frame.value); return; }
    const pending = this.pending;
    if (!pending || frame.id !== pending.id || (frame.kind !== 255 && frame.kind !== pending.kind + 128)) throw new Error('Mismatched host response');
    this.pending = null;
    if (frame.value instanceof HostError) pending.reject(frame.value); else pending.resolve(frame.value);
  }
  fail(error) {
    if (this.failure) return;
    this.failure = error; this.rejectReady(error);
    if (this.pending) { this.pending.reject(error); this.pending = null; }
    this.input.queue = []; this.worker.terminate();
  }
  request(kind, values) {
    if (this.failure) return Promise.reject(this.failure);
    if (this.queued >= 64) return Promise.reject(new Error('Too many queued host requests'));
    this.queued++;
    const result = this.tail.then(async () => {
      await this.ready;
      if (this.failure) throw this.failure;
      if (this.nextId > 0xffffffff) throw new Error('Request identifiers exhausted; restart the session');
      const id = this.nextId++, bytes = encodeRequest(kind, id, values);
      return new Promise((resolve, reject) => {
        this.pending = { id, kind, resolve, reject };
        try { this.input.write(bytes); } catch (error) { this.fail(error); }
      });
    });
    this.tail = result.catch(() => {}).finally(() => { this.queued--; });
    return result;
  }
  async hello() {
    const result = await this.request(1, { version: 1 });
    if (result.version !== 1) { const error = new Error('Unsupported host protocol version'); this.fail(error); throw error; }
    return result;
  }
  open(bytes, recovery = 1, password = new Uint8Array()) { return this.request(2, { bytes, recovery, password }); }
  pageInfo(document, page) { return this.request(3, { document, page }); }
  renderTile(document, page, scale, tile) { return this.request(4, { document, page, scale, ...tile }); }
  diagnostics(document) { return this.request(5, { document }); }
  close(document) { return this.request(6, { document }); }
  destroy() { this.fail(new Error('Host session closed')); }
}
