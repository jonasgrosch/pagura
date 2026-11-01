import {createInput, InputWriter} from '/web/transport.mjs';

/** One worker and the viewer's existing bounded WASI mailbox transport. */
export class Engine {
  static async create() {
    if (!crossOriginIsolated) throw new Error('The demo server must enable COOP/COEP');
    const engine = new Engine();
    await engine.ready;
    return engine;
  }

  constructor() {
    this.worker = new Worker(new URL('./worker.mjs', import.meta.url), {type: 'module'});
    this.input = new InputWriter(createInput());
    this.chunks = []; this.length = 0; this.pending = null; this.stderr = '';
    this.ready = new Promise((resolve, reject) => { this.resolveReady = resolve; this.rejectReady = reject; });
    this.worker.addEventListener('error', error => this.fail(new Error(error.message)));
    this.worker.addEventListener('message', ({data}) => {
      if (data.type === 'ready') this.resolveReady();
      else if (data.type === 'drain') this.input.pump();
      else if (data.type === 'stdout') { this.chunks.push(data.bytes); this.length += data.bytes.length; this.receive(); }
      else if (data.type === 'stderr') this.stderr = (this.stderr + new TextDecoder().decode(data.bytes)).slice(-2048);
      else if (data.type === 'fatal' || data.type === 'exit') this.fail(new Error(data.message || `Engine exited: ${data.code}; ${this.stderr}`));
    });
    this.worker.postMessage({type: 'start', shared: this.input.control.buffer});
  }

  fail(error) {
    this.failed = error;
    this.rejectReady(error); this.pending?.reject(error); this.pending = null;
  }

  receive() {
    if (this.length < 4) return;
    const header = new Uint8Array(4);
    let offset = 0;
    for (const chunk of this.chunks) { const n = Math.min(4 - offset, chunk.length); header.set(chunk.subarray(0, n), offset); offset += n; if (offset === 4) break; }
    const size = new DataView(header.buffer).getUint32(0, true);
    if (size > 68 * 1024 * 1024) { this.fail(new Error('Engine response exceeds demo budget')); this.destroy(); return; }
    if (this.length < size + 4) return;
    const frame = new Uint8Array(this.length);
    offset = 0;
    for (const chunk of this.chunks) { frame.set(chunk, offset); offset += chunk.length; }
    this.chunks = []; this.length = 0;
    const pending = this.pending; this.pending = null;
    if (!pending || frame.length !== size + 4) {
      const error = new Error('Unexpected engine response'); pending?.reject(error); this.fail(error); this.worker.terminate(); return;
    }
    const metaLength = new DataView(frame.buffer).getUint32(4, true);
    try {
      if (metaLength > size - 4) throw new Error('Truncated engine metadata');
      const meta = JSON.parse(new TextDecoder().decode(frame.subarray(8, 8 + metaLength)));
      if (!meta.ok) throw new Error(meta.error);
      pending.resolve({meta, data: frame.subarray(8 + metaLength), host_ms: performance.now() - pending.start});
    } catch (error) { pending.reject(error); }
  }

  request(bytes) {
    if (this.failed) throw this.failed;
    if (this.pending) throw new Error('One engine request may run at a time');
    return new Promise((resolve, reject) => {
      this.pending = {resolve, reject, start: performance.now()};
      const header = new Uint8Array(4); new DataView(header.buffer).setUint32(0, bytes.length, true);
      this.input.write(header); this.input.write(bytes);
    });
  }

  open(pdf) {
    this.generation = (this.generation ?? 0) + 1;
    if (pdf.length > 64 * 1024 * 1024 - 4) throw new Error('PDF exceeds the 64 MiB demo limit');
    const input = new Uint8Array(pdf.length + 4); new DataView(input.buffer).setUint32(0, 1, true); input.set(pdf, 4);
    return this.request(input);
  }

  render(operation, {page = 0, dpi = 150, band = 32, region = [0, 0, 0, 0]} = {}) {
    this.renderRequests = (this.renderRequests ?? 0) + 1;
    const bytes = new Uint8Array(32), view = new DataView(bytes.buffer);
    [operation, page, dpi, band, ...region].forEach((word, index) => view.setUint32(index * 4, word, true));
    return this.request(bytes);
  }

  destroy() { this.input.end(); this.worker.terminate(); this.fail(new Error('Engine closed')); }
}
