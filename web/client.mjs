// @ts-check
import { HostFailure } from './failure.mjs';
import { encodeRequest, FrameDecoder, HostError, VERSION_1_4 } from './protocol.mjs';
import { createInput, InputWriter } from './transport.mjs';
/**
 * @import { Diagnostic, Frame, GenerationTile, PageInfo, RequestValues, RetentionStats, Tile, TileRect } from './protocol.mjs'
 * @import { ContentBounds, Link, OutlineItem, PageText, ReadingStats, SearchResult } from './reading_protocol.mjs'
 * @import { HostMessage } from './worker.mjs'
 */

/**
 * The part of a Worker the session uses; tests substitute worker_threads or stand-ins.
 * The event shape depends on the type: `{ data: HostMessage }` for message, `{ message?: string }` for error.
 * @typedef {{
 *   addEventListener(type: 'message' | 'error' | 'messageerror', listener: (event: any) => void): void,
 *   postMessage(message: { type: 'start', shared: ArrayBufferLike }): void,
 *   terminate(): void,
 * }} HostWorker
 */
/**
 * @typedef {{ cancelled: true }} Cancelled A request the client skipped because a newer one replaced it.
 * @typedef {(SearchResult & { cancelled?: false }) | Cancelled} SearchResponse
 * @typedef {{
 *   1: { version: number }, 2: { document: number, pages: number }, 3: PageInfo, 4: Tile, 5: Diagnostic[],
 *   6: null, 7: { generation: number } | GenerationTile, 8: GenerationTile, 9: RetentionStats, 10: PageText,
 *   11: SearchResponse, 12: Link[], 13: OutlineItem[], 14: string[], 15: ReadingStats, 16: ContentBounds,
 *   17: Diagnostic[],
 * }} Responses Resolved value per request kind; see crates/pagura-host/PROTOCOL.md.
 * @typedef {{ id: number, kind: number, resolve: (value: any) => void, reject: (reason: unknown) => void }} Pending
 */

/** Long-lived serial session. Recreate the client after a trap/terminal failure. */
export class SessionClient {
  /**
   * @param {HostWorker} worker
   * @param {{ onFailure?: (error: unknown) => void, timeoutMs?: number, debug?: boolean }} [options]
   */
  constructor(worker, { onFailure = () => {}, timeoutMs = 30000, debug = false } = {}) {
    this.onFailure = onFailure;
    this.timeoutMs = timeoutMs;
    this.debug = debug;
    this.wasmBytes = 0;
    this.timer = setTimeout(
      () => this.fail(new HostFailure('deadline exceeded', 'startup exceeded its time budget')),
      timeoutMs,
    );
    this.worker = worker;
    this.input = new InputWriter(createInput());
    this.nextId = 1;
    this.searchEpoch = 0;
    /** @type {Map<number, number>} Latest viewport generation per document. */
    this.generations = new Map();
    /** @type {Map<number, number>} Generation last sent to the host per document. */
    this.selectedGenerations = new Map();
    /** @type {Promise<void>} */
    this.tail = Promise.resolve();
    /** @type {Pending | null} */
    this.pending = null;
    /** @type {unknown} */
    this.failure = null;
    this.queued = 0;
    this.decoder = new FrameDecoder((frame) => this.receive(frame));
    /** @type {Promise<void>} */
    this.ready = new Promise((resolve, reject) => {
      this.resolveReady = resolve;
      this.rejectReady = reject;
    });
    // Keep worker startup failures observed even before the first request.
    this.ready.catch(() => {});
    worker.addEventListener('message', (/** @type {{ data: HostMessage }} */ { data }) => {
      if (this.failure) return;
      try {
        if (data.type === 'ready') {
          clearTimeout(this.timer);
          this.wasmBytes = data.wasmBytes ?? 0;
          this.resolveReady();
        } else if (data.type === 'memory') this.wasmBytes = data.wasmBytes;
        else if (data.type === 'drain') this.input.pump();
        else if (data.type === 'stdout') this.decoder.feed(data.bytes);
        else if (data.type === 'fatal') this.fail(new HostFailure(data.reason ?? 'execution failure', data.message));
        else if (data.type === 'exit') {
          this.decoder.finish();
          this.fail(new Error(`Host session exited (${data.code})`));
        }
      } catch (error) {
        this.fail(error);
      }
    });
    worker.addEventListener('error', (/** @type {{ message?: string }} */ event) =>
      this.fail(new HostFailure('execution failure', event.message || 'Host execution failed')),
    );
    worker.addEventListener('messageerror', () => this.fail(new Error('Invalid worker message')));
    worker.postMessage({ type: 'start', shared: this.input.control.buffer });
  }
  /** @param {Frame} frame */
  receive(frame) {
    if (frame.kind === 255 && frame.id === 0) {
      this.fail(frame.value);
      return;
    }
    const pending = this.pending;
    if (!pending || frame.id !== pending.id || (frame.kind !== 255 && frame.kind !== pending.kind + 128))
      throw new Error('Mismatched host response');
    if (frame.value instanceof HostError && frame.value.category === 2) {
      this.fail(new HostFailure('budget exceeded', frame.value));
      return;
    }
    clearTimeout(this.timer);
    this.pending = null;
    if (frame.value instanceof HostError) pending.reject(frame.value);
    else pending.resolve(frame.value);
  }
  /**
   * Terminate the session; every pending and later request rejects with `error`.
   * @param {unknown} error
   */
  fail(error, notify = true) {
    if (this.failure) return;
    clearTimeout(this.timer);
    this.failure = error;
    this.rejectReady(error);
    if (this.pending) {
      this.pending.reject(error);
      this.pending = null;
    }
    this.input.queue = [];
    this.input.offset = 0;
    this.decoder = new FrameDecoder(() => {});
    this.worker.terminate();
    if (notify) queueMicrotask(() => this.onFailure(error));
  }
  /**
   * Queue one request; requests run strictly in order, one at a time.
   * @template {keyof Responses} K
   * @param {K} kind
   * @param {RequestValues} [values] Required by every kind except 9 and 15.
   * @returns {Promise<Responses[K]>}
   */
  request(kind, values) {
    if (this.failure) return Promise.reject(this.failure);
    if (this.queued >= 64) return Promise.reject(new Error('Too many queued host requests'));
    this.queued++;
    const result = this.tail.then(async () => {
      await this.ready;
      if (this.failure) throw this.failure;
      const args = /** @type {RequestValues} */ (values);
      if (kind === 11 && args.token !== undefined && args.token !== this.searchEpoch)
        return /** @type {Responses[K]} */ ({ cancelled: true });
      if (
        (kind === 7 || kind === 8) &&
        this.generations.get(/** @type {number} */ (args.document)) !== args.generation
      ) {
        return /** @type {Responses[K]} */ ({ cancelled: true, generation: args.generation });
      }
      if (this.nextId > 0xffffffff) throw new Error('Request identifiers exhausted; restart the session');
      const id = this.nextId++,
        bytes = encodeRequest(kind, id, values);
      return /** @type {Promise<Responses[K]>} */ (
        new Promise((resolve, reject) => {
          this.pending = { id, kind, resolve, reject };
          this.timer = setTimeout(
            () => this.fail(new HostFailure('deadline exceeded', 'request exceeded its time budget')),
            this.timeoutMs,
          );
          try {
            this.input.write(bytes);
          } catch (error) {
            this.fail(error);
          }
        })
      );
    });
    this.tail = result
      .then(
        () => {},
        () => {},
      )
      .finally(() => {
        this.queued--;
      });
    return result;
  }
  async hello(version = VERSION_1_4) {
    const result = await this.request(1, { version });
    if (result.version !== version) {
      const error = new Error('Unsupported host protocol version');
      this.fail(error);
      throw error;
    }
    /** @type {number | undefined} The version hello() negotiated. */
    this.version = version;
    return result;
  }
  /**
   * @param {Uint8Array} bytes
   * @param {number} [recovery] 1 repairs and reports malformed input; 0 is strict.
   * @param {Uint8Array} [password]
   */
  open(bytes, recovery = 1, password = new Uint8Array()) {
    return this.request(2, { bytes, recovery, password });
  }
  /**
   * @param {number} document
   * @param {number} page
   */
  pageInfo(document, page) {
    return this.request(3, { document, page });
  }
  /**
   * @param {number} document
   * @param {number} page
   */
  contentBounds(document, page) {
    return this.request(16, { document, page });
  }
  /**
   * @param {number} document
   * @param {number} page
   * @param {number} scale Device pixels per PDF unit.
   * @param {TileRect} tile
   */
  renderTile(document, page, scale, tile) {
    return this.request(4, { document, page, scale, ...tile });
  }
  /**
   * Start a new viewport generation; older queued generation tiles are skipped.
   * @param {number} document
   */
  invalidate(document) {
    const generation = (this.generations.get(document) ?? 0) + 1;
    if (generation > 0xffffffff) throw new Error('Viewport generations exhausted; reopen the document');
    this.generations.set(document, generation);
    return generation;
  }
  /**
   * @param {number} document
   * @param {number} page
   * @param {number} scale
   * @param {TileRect} tile
   * @param {number} generation From invalidate.
   * @returns {Promise<GenerationTile>}
   */
  async renderGenerationTile(document, page, scale, tile, generation) {
    if (this.generations.get(document) !== generation) return { cancelled: true, generation };
    if (this.selectedGenerations.get(document) !== generation) {
      this.selectedGenerations.set(document, generation);
      await this.request(7, { document, generation });
    }
    return this.request(8, { document, page, scale, ...tile, generation });
  }
  /**
   * @param {number} document
   * @param {number} page
   */
  pageText(document, page) {
    return this.request(10, { document, page });
  }
  invalidateSearch() {
    return ++this.searchEpoch;
  }
  /**
   * @param {number} document
   * @param {string} query
   * @param {number} start First page.
   * @param {number} end Page after the last.
   * @param {number} [flags] Bit 0 case-sensitive, 1 whole word, 2 fold whitespace and hyphens.
   * @param {number} [token] Search epoch; an older token resolves as cancelled.
   */
  search(document, query, start, end, offset = 0, limit = 128, flags = 4, token = this.searchEpoch) {
    return this.request(11, { document, query, start, end, offset, limit, flags, token });
  }
  /**
   * @param {number} document
   * @param {number} page
   */
  links(document, page) {
    return this.request(12, { document, page });
  }
  /** @param {number} document */
  outlines(document) {
    return this.request(13, { document });
  }
  /** @param {number} document */
  labels(document) {
    return this.request(14, { document });
  }
  readingStats() {
    return this.request(15);
  }
  stats() {
    return this.request(9);
  }
  /**
   * Development injection stays in the client; no engine instruction changes.
   * @param {string} reason `trap`, `memory`, `budget` or `deadline`.
   */
  injectFailure(reason) {
    if (!this.debug) throw new Error('Failure injection is disabled');
    const failures = {
      trap: new WebAssembly.RuntimeError('injected trap'),
      memory: new RangeError('injected memory exhaustion'),
      budget: new HostError(2, '', '', 'injected budget exhaustion', {}),
      deadline: new Error('injected deadline'),
    };
    if (!Object.hasOwn(failures, reason)) throw new Error('Unknown injected failure');
    const names = { trap: 'trap', memory: 'memory failure', budget: 'budget exceeded', deadline: 'deadline exceeded' };
    const key = /** @type {keyof typeof failures} */ (reason);
    this.fail(new HostFailure(names[key], failures[key]));
  }
  /** @param {number} document */
  diagnostics(document) {
    return this.request(this.version === VERSION_1_4 ? 17 : 5, { document });
  }
  /** @param {number} document */
  close(document) {
    this.generations.delete(document);
    this.selectedGenerations.delete(document);
    return this.request(6, { document });
  }
  destroy() {
    this.fail(new Error('Host session closed'), false);
  }
}
