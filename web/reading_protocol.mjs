import { ActionCode, FitCode, MAX_QUERY_BYTES, MAX_READING_RESPONSE, MAX_SEARCH_MATCHES, OutlineStyle, RequestKind, ResponseKind, SearchFlags } from './protocol_constants.mjs';
/** Additive reading and content-bounds layouts. Counts precede allocations. */
function list(r, minimum, read, max = 1000000) {
  const count = r.u32();
  if (count > max || count > Math.floor((r.bytes.length - r.offset) / minimum)) throw new Error('Invalid reading record count');
  return Array.from({ length: count }, read);
}
function quad(r) { const q = Array.from({ length: 8 }, () => r.f64()); if (!q.every(Number.isFinite)) throw new Error('Nonfinite quad'); return q; }
function target(r) {
  const actions = list(r, 5, () => {
    const parent = r.u32(), kind = r.u8(); let value;
    if (kind === ActionCode.GoTo) {
      const page = r.u32(), fit = r.u8(), mask = r.u8();
      const masks = [7, 0, 1, 1, 15, 0, 1, 1];
      if (fit > FitCode.FitBv || (mask & ~masks[fit]) || (fit === FitCode.FitR && mask !== 15)) throw new Error('Invalid destination');
      const coordinates = Array.from({ length: 4 }, (_, i) => mask & (1 << i) ? r.f64() : null);
      if (!coordinates.every((v) => v === null || Number.isFinite(v))) throw new Error('Nonfinite destination');
      value = { page, fit, coordinates };
    } else if (kind === ActionCode.Uri) {
      const uri = r.text(), present = r.u8(); if (present > 1) throw new Error('Invalid URI base flag');
      const base = present ? r.text() : null, isMap = r.u8(); if (isMap > 1) throw new Error('Invalid URI map flag');
      value = { uri, base, isMap: Boolean(isMap) };
    } else if (kind === ActionCode.Named || kind === ActionCode.Other) value = { name: r.text() };
    else if (kind < ActionCode.GoToR || kind > ActionCode.JavaScript) throw new Error('Unknown action type');
    return { parent, kind, ...value };
  });
  for (const [i, action] of actions.entries()) if (action.parent > i) throw new Error('Invalid action parent');
  return actions;
}
export function decodeReading(kind, r) {
  if (r.bytes.length > MAX_READING_RESPONSE + 5) throw new Error('Reading response budget exceeded');
  if (kind === ResponseKind.PageText) {
    const text = r.text(), bytes = new TextEncoder().encode(text), quads = list(r, 64, () => quad(r)); let end = 0;
    const characters = list(r, 12, () => {
      const start = end, next = r.u32(), quadStart = r.u32(), quadCount = r.u32();
      if (next <= start || next > bytes.length || (bytes[next] & 0xc0) === 0x80) throw new Error('Invalid character byte range');
      if (quadStart > quads.length - quadCount) throw new Error('Invalid character quad range');
      end = next; return { start, end, quadStart, quadCount };
    });
    if (end !== bytes.length) throw new Error('Incomplete character ranges');
    return { text, quads, characters };
  }
  if (kind === ResponseKind.Search) {
    const nextPage = r.u32(), nextOffset = r.u32(), done = r.u8(); if (done > 1) throw new Error('Invalid search status');
    const matches = list(r, 16, () => {
      const page = r.u32(), start = r.u32(), end = r.u32(); if (end <= start) throw new Error('Invalid match range');
      return { page, start, end, quads: list(r, 64, () => quad(r)) };
    }, MAX_SEARCH_MATCHES);
    return { nextPage, nextOffset, done: Boolean(done), matches };
  }
  if (kind === ResponseKind.Links) return list(r, 8, () => ({ quads: list(r, 64, () => quad(r)), target: target(r) }));
  if (kind === ResponseKind.Outlines) {
    let previous = -1;
    return list(r, 34, () => {
      const depth = r.u32(), title = r.text(), open = r.u8(), style = r.u8();
      const color = Array.from({ length: 3 }, () => r.f64());
      if (depth > previous + 1 || open > 1 || (style & ~(OutlineStyle.ITALIC | OutlineStyle.BOLD)) || !color.every((c) => Number.isFinite(c) && c >= 0 && c <= 1)) throw new Error('Invalid outline record');
      previous = depth; return { depth, title, open: Boolean(open), style, color, target: target(r) };
    });
  }
  if (kind === ResponseKind.Labels) return list(r, 4, () => r.text(), 200000);
  if (kind === ResponseKind.ReadingStats) return { sourceBytes: r.offset64(), cacheBytes: r.offset64(), documents: r.u32(), lists: r.u32(),
    textBytes: r.offset64(), textPages: r.u32() };
  if (kind === ResponseKind.Bounds) {
    const present = r.u8(); if (present > 1) throw new Error('Invalid content bounds flag');
    const bounds = present ? Array.from({length:4}, () => r.f64()) : null;
    if (bounds && (!bounds.every(Number.isFinite) || bounds[0] > bounds[2] || bounds[1] > bounds[3])) throw new Error('Invalid content bounds');
    return { bounds };
  }
  throw new Error('Unknown reading response');
}
export function encodeReading(kind, values) {
  if (kind === RequestKind.ReadingStats) return new Uint8Array();
  const query = kind === RequestKind.Search ? new TextEncoder().encode(values.query) : null;
  if (query && query.length > MAX_QUERY_BYTES) throw new Error('Search query budget exceeded');
  const bytes = new Uint8Array(kind === RequestKind.Search ? 25 + query.length : kind === RequestKind.PageText || kind === RequestKind.Links || kind === RequestKind.Bounds ? 8 : 4);
  const view = new DataView(bytes.buffer); let offset = 0;
  const u32 = (n) => { if (!Number.isInteger(n) || n < 0 || n > 0xffffffff) throw new Error('Expected u32 reading argument'); view.setUint32(offset, n, true); offset += 4; };
  u32(values.document);
  if (kind === RequestKind.PageText || kind === RequestKind.Links || kind === RequestKind.Bounds) u32(values.page);
  if (kind === RequestKind.Search) {
    for (const field of ['start', 'end', 'offset', 'limit']) u32(values[field]);
    if (!Number.isInteger(values.flags) || values.flags < 0 || (values.flags & ~(SearchFlags.CASE_SENSITIVE | SearchFlags.WHOLE_WORDS | SearchFlags.JOIN_HYPHENATED))) throw new Error('Invalid search flags');
    bytes[offset++] = values.flags; u32(query.length); bytes.set(query, offset);
  }
  return bytes;
}
