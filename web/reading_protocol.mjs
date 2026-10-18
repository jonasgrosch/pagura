// @ts-check
/** @import { ProtocolReader, RequestValues } from './protocol.mjs' */
/**
 * @typedef {number[]} Quad Eight finite coordinates: four points in PDF user space.
 * @typedef {{ start: number, end: number, quadStart: number, quadCount: number }} Character UTF-8 byte range and quads.
 * @typedef {{ text: string, quads: Quad[], characters: Character[] }} PageText
 * @typedef {{ page: number, start: number, end: number, quads: Quad[] }} Match
 * @typedef {{ nextPage: number, nextOffset: number, done: boolean, matches: Match[] }} SearchResult
 * @typedef {{ parent: number, kind: 1, page: number, fit: number, coordinates: (number | null)[] }} GoToAction
 * @typedef {{ parent: number, kind: 2, uri: string, base: string | null, isMap: boolean }} UriAction
 * @typedef {{ parent: number, kind: 3 | 7, name: string }} NamedAction Kind 3 Named; 7 any other type, by name.
 * @typedef {{ parent: number, kind: 4 | 5 | 6 }} InertAction Kind 4 GoToR, 5 Launch, 6 JavaScript.
 * @typedef {GoToAction | UriAction | NamedAction | InertAction} Action
 * @typedef {Action[]} Target Actions in engine preorder; empty when a link has no target.
 * @typedef {{ quads: Quad[], target: Target }} Link
 * @typedef {{ depth: number, title: string, open: boolean, style: number, color: number[], target: Target }} OutlineItem
 * @typedef {{ sourceBytes: number, cacheBytes: number, documents: number, lists: number,
 *   textBytes: number, textPages: number }} ReadingStats
 * @typedef {{ bounds: number[] | null }} ContentBounds Painted-content rectangle, null for an empty page.
 * @typedef {PageText | SearchResult | Link[] | OutlineItem[] | string[] | ReadingStats | ContentBounds} ReadingValue
 */

/**
 * Additive reading and content-bounds layouts. Counts precede allocations.
 * @template T
 * @param {ProtocolReader} r
 * @param {number} minimum Smallest encoded record size, bounding the count by the remaining bytes.
 * @param {() => T} read
 * @returns {T[]}
 */
function list(r, minimum, read, max = 1000000) {
  const count = r.u32();
  if (count > max || count > Math.floor((r.bytes.length - r.offset) / minimum))
    throw new Error('Invalid reading record count');
  return Array.from({ length: count }, read);
}
/**
 * @param {ProtocolReader} r
 * @returns {Quad}
 */
function quad(r) {
  const q = Array.from({ length: 8 }, () => r.f64());
  if (!q.every(Number.isFinite)) throw new Error('Nonfinite quad');
  return q;
}
/**
 * @param {ProtocolReader} r
 * @returns {Target}
 */
function target(r) {
  const actions = list(r, 5, () => {
    const parent = r.u32(),
      kind = r.u8();
    let value;
    if (kind === 1) {
      const page = r.u32(),
        fit = r.u8(),
        mask = r.u8();
      const masks = [7, 0, 1, 1, 15, 0, 1, 1];
      if (fit > 7 || mask & ~masks[fit] || (fit === 4 && mask !== 15)) throw new Error('Invalid destination');
      const coordinates = Array.from({ length: 4 }, (_, i) => (mask & (1 << i) ? r.f64() : null));
      if (!coordinates.every((v) => v === null || Number.isFinite(v))) throw new Error('Nonfinite destination');
      value = { page, fit, coordinates };
    } else if (kind === 2) {
      const uri = r.text(),
        present = r.u8();
      if (present > 1) throw new Error('Invalid URI base flag');
      const base = present ? r.text() : null,
        isMap = r.u8();
      if (isMap > 1) throw new Error('Invalid URI map flag');
      value = { uri, base, isMap: Boolean(isMap) };
    } else if (kind === 3 || kind === 7) value = { name: r.text() };
    else if (kind < 4 || kind > 6) throw new Error('Unknown action type');
    // The checks above admit exactly the fields of the kind's Action variant.
    return /** @type {Action} */ ({ parent, kind, ...value });
  });
  for (const [i, action] of actions.entries()) if (action.parent > i) throw new Error('Invalid action parent');
  return actions;
}
/**
 * @param {number} kind Response kind 138-144.
 * @param {ProtocolReader} r
 * @returns {ReadingValue}
 */
export function decodeReading(kind, r) {
  if (r.bytes.length > 32 * 1024 * 1024 + 5) throw new Error('Reading response budget exceeded');
  if (kind === 138) {
    const text = r.text(),
      bytes = new TextEncoder().encode(text),
      quads = list(r, 64, () => quad(r));
    let end = 0;
    const characters = list(r, 12, () => {
      const start = end,
        next = r.u32(),
        quadStart = r.u32(),
        quadCount = r.u32();
      if (next <= start || next > bytes.length || (bytes[next] & 0xc0) === 0x80)
        throw new Error('Invalid character byte range');
      if (quadStart > quads.length - quadCount) throw new Error('Invalid character quad range');
      end = next;
      return { start, end, quadStart, quadCount };
    });
    if (end !== bytes.length) throw new Error('Incomplete character ranges');
    return { text, quads, characters };
  }
  if (kind === 139) {
    const nextPage = r.u32(),
      nextOffset = r.u32(),
      done = r.u8();
    if (done > 1) throw new Error('Invalid search status');
    const matches = list(
      r,
      16,
      () => {
        const page = r.u32(),
          start = r.u32(),
          end = r.u32();
        if (end <= start) throw new Error('Invalid match range');
        return { page, start, end, quads: list(r, 64, () => quad(r)) };
      },
      1024,
    );
    return { nextPage, nextOffset, done: Boolean(done), matches };
  }
  if (kind === 140) return list(r, 8, () => ({ quads: list(r, 64, () => quad(r)), target: target(r) }));
  if (kind === 141) {
    let previous = -1;
    return list(r, 34, () => {
      const depth = r.u32(),
        title = r.text(),
        open = r.u8(),
        style = r.u8();
      const color = Array.from({ length: 3 }, () => r.f64());
      if (depth > previous + 1 || open > 1 || style > 3 || !color.every((c) => Number.isFinite(c) && c >= 0 && c <= 1))
        throw new Error('Invalid outline record');
      previous = depth;
      return { depth, title, open: Boolean(open), style, color, target: target(r) };
    });
  }
  if (kind === 142) return list(r, 4, () => r.text(), 200000);
  if (kind === 143)
    return {
      sourceBytes: r.offset64(),
      cacheBytes: r.offset64(),
      documents: r.u32(),
      lists: r.u32(),
      textBytes: r.offset64(),
      textPages: r.u32(),
    };
  if (kind === 144) {
    const present = r.u8();
    if (present > 1) throw new Error('Invalid content bounds flag');
    const bounds = present ? Array.from({ length: 4 }, () => r.f64()) : null;
    if (bounds && (!bounds.every(Number.isFinite) || bounds[0] > bounds[2] || bounds[1] > bounds[3]))
      throw new Error('Invalid content bounds');
    return { bounds };
  }
  throw new Error('Unknown reading response');
}
/**
 * @param {number} kind Request kind 10-16.
 * @param {RequestValues} values
 */
export function encodeReading(kind, values) {
  if (kind === 15) return new Uint8Array();
  const query = kind === 11 ? new TextEncoder().encode(values.query) : null;
  if (query && query.length > 4096) throw new Error('Search query budget exceeded');
  // Only kind 11 reads `search`, and kind 11 always encodes a query.
  const search = /** @type {Uint8Array} */ (query);
  const bytes = new Uint8Array(kind === 11 ? 25 + search.length : kind === 10 || kind === 12 || kind === 16 ? 8 : 4);
  const view = new DataView(bytes.buffer);
  let offset = 0;
  /** @param {unknown} n */
  const u32 = (n) => {
    if (!Number.isInteger(n) || /** @type {number} */ (n) < 0 || /** @type {number} */ (n) > 0xffffffff)
      throw new Error('Expected u32 reading argument');
    view.setUint32(offset, /** @type {number} */ (n), true);
    offset += 4;
  };
  u32(values.document);
  if (kind === 10 || kind === 12 || kind === 16) u32(values.page);
  if (kind === 11) {
    for (const field of /** @type {const} */ (['start', 'end', 'offset', 'limit'])) u32(values[field]);
    const flags = /** @type {number} */ (values.flags);
    if (!Number.isInteger(flags) || flags < 0 || flags > 7) throw new Error('Invalid search flags');
    bytes[offset++] = flags;
    u32(search.length);
    bytes.set(search, offset);
  }
  return bytes;
}
