import { MAX_FILE } from './protocol_constants.mjs';

const BLOCK = 65536;
const closed = () => new DOMException('HTTP source closed', 'AbortError');
function integer(value, name, maximum = Number.MAX_SAFE_INTEGER) {
  if (!Number.isSafeInteger(value) || value <= 0 || value > maximum) throw new Error(`Invalid HTTP source ${name}`);
  return value;
}
function decimal(value) {
  if (value === null || !/^(0|[1-9][0-9]*)$/.test(value)) throw new Error('Invalid HTTP source length');
  const result = Number(value);
  if (!Number.isSafeInteger(result)) throw new Error('HTTP source length exceeds exact offsets');
  return result;
}
function contentRange(response) {
  const match = /^bytes (0|[1-9][0-9]*)-(0|[1-9][0-9]*)\/(\*|0|[1-9][0-9]*)$/.exec(response.headers.get('content-range') ?? '');
  if (!match) throw new Error('Invalid HTTP Content-Range');
  const start = decimal(match[1]), end = decimal(match[2]) + 1, length = match[3] === '*' ? null : decimal(match[3]);
  if (!Number.isSafeInteger(end) || start >= end || (length !== null && end > length)) throw new Error('Invalid HTTP range extent');
  return { start, end, length };
}
function strongTag(response) {
  const value = response.headers.get('etag');
  // RFC 9110 8.8.3: exclude weak tags, quote characters and controls.
  return value !== null && /^"[\x21\x23-\x7e\x80-\xff]*"$/.test(value) ? value : null;
}
function identity(response) {
  const encoding = response.headers.get('content-encoding');
  // CORS hides unexposed headers. A cross-origin range endpoint must expose
  // Content-Encoding: identity; absence alone cannot prove no decoding occurred.
  return encoding === 'identity' || (encoding === null && response.type !== 'cors');
}

/** An explicitly authorized HTTP URL becomes one immutable driver registration.
 * Strong-ETag identity ranges follow RFC 9110 sections 8.8 and 14. Unknown length,
 * hidden/weak validators and content coding use one bounded complete response.
 * Redirects, URL credentials and multipart ranges are refused. Credentials and
 * headers are application-owned; no URL can originate in a PDF object.
 *
 * At most `requests` responses are live. Range collection retains at most
 * `rangeBytes` per response plus the probe. Complete spooling retains at most
 * twice `spoolBytes` while joining fixed-size blocks; a browser's network/stream
 * buffers and the driver's supplied copies are separate allocations. Retries and
 * overfetch are zero. Body-read counts, `transferBytes`, request deadlines and idle expiry
 * are finite; failed representation checks invalidate this registration.
 */
export async function httpSource(url, {
  fetcher = globalThis.fetch, headers, credentials = 'omit', signal,
  maxBytes = MAX_FILE, rangeBytes = Math.min(maxBytes, 1024 * 1024), probeBytes = Math.min(rangeBytes, BLOCK),
  spoolBytes = Math.min(maxBytes, 16 * 1024 * 1024), transferBytes = MAX_FILE * 2,
  requests = 4, bodyChunks = 65536, timeoutMs = 15000, idleMs = 120000,
} = {}) {
  url = new URL(url);
  if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || url.hash) throw new Error('Invalid authorized HTTP source URL');
  if (!['omit', 'same-origin', 'include'].includes(credentials) || typeof fetcher !== 'function') throw new Error('Invalid HTTP source transport');
  integer(maxBytes, 'file limit', MAX_FILE); integer(rangeBytes, 'range limit', maxBytes);
  integer(probeBytes, 'probe limit', rangeBytes); integer(spoolBytes, 'spool limit', maxBytes);
  integer(transferBytes, 'transfer limit'); integer(requests, 'request limit');
  integer(bodyChunks, 'body-read limit');
  integer(timeoutMs, 'request deadline', 0x7fffffff); integer(idleMs, 'idle lifetime', 0x7fffffff);
  const original = new Headers(headers);
  for (const name of ['range', 'if-range', 'if-match', 'if-none-match', 'accept-encoding']) {
    if (original.has(name)) throw new Error('HTTP source controls range and representation headers');
  }
  const controller = new AbortController(), version = crypto.getRandomValues(new Uint8Array(32));
  let idle, active = 0, transferred = 0, length, etag, retained = null, complete = false;
  const abort = () => stop(signal.reason);
  function stop(reason = closed()) {
    clearTimeout(idle); retained = null;
    if (!controller.signal.aborted) controller.abort(reason);
    signal?.removeEventListener('abort', abort);
  }
  function alive() { controller.signal.throwIfAborted(); }
  function touch() {
    alive(); clearTimeout(idle);
    if (!active) idle = setTimeout(() => stop(new DOMException('HTTP source idle lifetime exceeded', 'TimeoutError')), idleMs);
  }
  signal?.addEventListener('abort', abort, { once: true });
  if (signal?.aborted) abort();

  async function collect(response, bound, expected) {
    const declared = response.headers.get('content-length');
    if (identity(response) && declared !== null) {
      const count = decimal(declared);
      if (count > bound || (expected !== undefined && count !== expected)) throw new Error('HTTP response length exceeds its admitted extent');
      expected = count;
    }
    if (!response.body) throw new Error('HTTP source response has no body');
    const reader = response.body.getReader(), blocks = [];
    let size = 0, block = null, used = 0, reads = 0;
    try {
      for (;;) {
        if (reads++ >= bodyChunks) throw new Error('HTTP source body-read budget exceeded');
        const { done, value } = await reader.read(); alive();
        if (done) break;
        if (!(value instanceof Uint8Array)) throw new Error('Invalid HTTP source body chunk');
        if (value.length > transferBytes - transferred) throw new Error('HTTP source transfer budget exceeded');
        transferred += value.length;
        if (value.length > bound - size || (expected !== undefined && value.length > expected - size)) throw new Error('HTTP response exceeds its admitted extent');
        for (let offset = 0; offset < value.length;) {
          if (block === null || used === block.length) {
            block = new Uint8Array(Math.min(expected ?? BLOCK, bound - size));
            blocks.push(block); used = 0;
          }
          const count = Math.min(block.length - used, value.length - offset);
          block.set(value.subarray(offset, offset + count), used);
          used += count; offset += count; size += count;
        }
      }
      if (expected !== undefined && size !== expected) throw new Error('Truncated HTTP source response');
      if (blocks.length === 1 && blocks[0].length === size) return blocks[0];
      const bytes = new Uint8Array(size);
      let offset = 0;
      for (const part of blocks) { const count = Math.min(part.length, size - offset); bytes.set(part.subarray(0, count), offset); offset += count; }
      return bytes;
    } catch (error) { void reader.cancel().catch(() => {}); throw error; }
    finally { reader.releaseLock(); }
  }

  async function request(range, consumer, consume) {
    alive(); consumer?.throwIfAborted();
    if (active >= requests) throw new Error('HTTP source request budget exceeded');
    active++; clearTimeout(idle);
    const deadline = new AbortController(), timer = setTimeout(() => deadline.abort(new DOMException('HTTP source request deadline exceeded', 'TimeoutError')), timeoutMs);
    const combined = AbortSignal.any([controller.signal, deadline.signal, ...(consumer ? [consumer] : [])]);
    let response;
    try {
      const fields = new Headers(original);
      if (range) { fields.set('range', `bytes=${range.start}-${range.end - 1}`); if (etag) fields.set('if-range', etag); }
      try { response = await fetcher(url.href, { method: 'GET', headers: fields, credentials, mode: 'cors', redirect: 'error', cache: 'no-store', signal: combined }); }
      catch { combined.throwIfAborted(); throw new Error('HTTP source request failed'); }
      combined.throwIfAborted();
      if (response.redirected || (response.url && response.url !== url.href) || ['opaque', 'opaqueredirect', 'error'].includes(response.type)) throw new Error('HTTP source response changed endpoint or visibility');
      const result = await consume(response);
      combined.throwIfAborted();
      return result;
    } catch (error) {
      combined.throwIfAborted(); throw error;
    } finally {
      void response?.body?.cancel().catch(() => {});
      clearTimeout(timer); active--;
      if (!active && !controller.signal.aborted) touch();
    }
  }

  try {
    const ranged = await request({ start: 0, end: probeBytes }, signal, async (response) => {
      if (response.status === 200) {
        if (response.headers.has('content-range')) throw new Error('HTTP spool did not return a complete response');
        retained = await collect(response, spoolBytes); length = retained.length; complete = true; return false;
      }
      if (response.status !== 206) throw new Error('HTTP source probe did not return a usable representation');
      const range = contentRange(response);
      if (range.start !== 0 || range.end > probeBytes) throw new Error('HTTP probe range does not match its request');
      if (range.length === null || !identity(response) || !strongTag(response)) return false;
      if (range.length > maxBytes) throw new Error('HTTP source exceeds its file budget');
      if (range.end !== Math.min(range.length, probeBytes)) throw new Error('HTTP probe range is incomplete');
      length = range.length; etag = strongTag(response);
      retained = await collect(response, probeBytes, range.end); return true;
    });
    if (!ranged && !complete) {
      retained = await request(null, signal, (response) => {
        if (response.status !== 200 || response.headers.has('content-range')) throw new Error('HTTP spool did not return a complete response');
        return collect(response, spoolBytes);
      });
      length = retained.length; complete = true;
    }
    alive();
  } catch (error) { stop(error); throw error; }

  return {
    length: BigInt(length), version: version.slice(), signal: controller.signal, mode: complete ? 'complete' : 'range',
    close: stop,
    async read({ start, end }, consumer) {
      alive(); consumer?.throwIfAborted();
      if (typeof start !== 'bigint' || typeof end !== 'bigint' || start < 0n || start >= end || end > BigInt(length) || end - start > BigInt(rangeBytes)) throw new Error('Invalid HTTP source read extent');
      start = Number(start); end = Number(end); touch();
      try {
        let bytes;
        if (end <= retained.length) bytes = retained.slice(start, end);
        else bytes = await request({ start, end }, consumer, async (response) => {
          // A late 200 is a complete representation, never bytes at `start`.
          // Invalidate this source; a new probe can admit its bounded full body.
          if (response.status !== 206) throw new Error('HTTP source stopped serving the registered byte ranges');
          const range = contentRange(response);
          if (!identity(response) || strongTag(response) !== etag || range.length !== length) throw new Error('HTTP source representation changed');
          if (range.start !== start || range.end !== end) throw new Error('HTTP response range does not match its request');
          return collect(response, rangeBytes, end - start);
        });
        alive(); consumer?.throwIfAborted();
        return { length: BigInt(length), version: version.slice(), offset: BigInt(start), bytes };
      } catch (error) {
        if (!consumer?.aborted) stop(error);
        throw error;
      }
    },
  };
}
