/** Additive v1.2 reading layouts. Counts are checked before allocating arrays. */
function list(r, minimum, read, max = 1000000) {
  const count = r.u32();
  if (count > max || count > Math.floor((r.bytes.length - r.offset) / minimum)) throw new Error('Invalid reading record count');
  return Array.from({ length: count }, read);
}
function quad(r) { const q = Array.from({ length: 8 }, () => r.f64()); if (!q.every(Number.isFinite)) throw new Error('Nonfinite quad'); return q; }
function target(r) {
  const actions = list(r, 5, () => {
    const parent = r.u32(), kind = r.u8(); let value;
    if (kind === 1) {
      const page = r.u32(), fit = r.u8(), mask = r.u8();
      const masks = [7, 0, 1, 1, 15, 0, 1, 1];
      if (fit > 7 || (mask & ~masks[fit]) || (fit === 4 && mask !== 15)) throw new Error('Invalid destination');
      const coordinates = Array.from({ length: 4 }, (_, i) => mask & (1 << i) ? r.f64() : null);
      if (!coordinates.every((v) => v === null || Number.isFinite(v))) throw new Error('Nonfinite destination');
      value = { page, fit, coordinates };
    } else if (kind === 2) {
      const uri = r.text(), present = r.u8(); if (present > 1) throw new Error('Invalid URI base flag');
      const base = present ? r.text() : null, isMap = r.u8(); if (isMap > 1) throw new Error('Invalid URI map flag');
      value = { uri, base, isMap: Boolean(isMap) };
    } else if (kind === 3 || kind === 7) value = { name: r.text() };
    else if (kind < 4 || kind > 6) throw new Error('Unknown action type');
    return { parent, kind, ...value };
  });
  for (const [i, action] of actions.entries()) if (action.parent > i) throw new Error('Invalid action parent');
  return actions;
}
export function decodeReading(kind, r) {
  if (r.bytes.length > 32 * 1024 * 1024 + 5) throw new Error('Reading response budget exceeded');
  if (kind === 138) {
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
  if (kind === 139) {
    const nextPage = r.u32(), nextOffset = r.u32(), done = r.u8(); if (done > 1) throw new Error('Invalid search status');
    const matches = list(r, 16, () => {
      const page = r.u32(), start = r.u32(), end = r.u32(); if (end <= start) throw new Error('Invalid match range');
      return { page, start, end, quads: list(r, 64, () => quad(r)) };
    }, 1024);
    return { nextPage, nextOffset, done: Boolean(done), matches };
  }
  if (kind === 140) return list(r, 8, () => ({ quads: list(r, 64, () => quad(r)), target: target(r) }));
  if (kind === 141) {
    let previous = -1;
    return list(r, 34, () => {
      const depth = r.u32(), title = r.text(), open = r.u8(), style = r.u8();
      const color = Array.from({ length: 3 }, () => r.f64());
      if (depth > previous + 1 || open > 1 || style > 3 || !color.every((c) => Number.isFinite(c) && c >= 0 && c <= 1)) throw new Error('Invalid outline record');
      previous = depth; return { depth, title, open: Boolean(open), style, color, target: target(r) };
    });
  }
  if (kind === 142) return list(r, 4, () => r.text(), 200000);
  if (kind === 143) return { sourceBytes: r.offset64(), cacheBytes: r.offset64(), documents: r.u32(), lists: r.u32(),
    textBytes: r.offset64(), textPages: r.u32() };
  throw new Error('Unknown reading response');
}
export function encodeReading(kind, values) {
  if (kind === 15) return new Uint8Array();
  const query = kind === 11 ? new TextEncoder().encode(values.query) : null;
  if (query && query.length > 4096) throw new Error('Search query budget exceeded');
  const bytes = new Uint8Array(kind === 11 ? 25 + query.length : kind === 10 || kind === 12 ? 8 : 4);
  const view = new DataView(bytes.buffer); let offset = 0;
  const u32 = (n) => { if (!Number.isInteger(n) || n < 0 || n > 0xffffffff) throw new Error('Expected u32 reading argument'); view.setUint32(offset, n, true); offset += 4; };
  u32(values.document);
  if (kind === 10 || kind === 12) u32(values.page);
  if (kind === 11) {
    for (const field of ['start', 'end', 'offset', 'limit']) u32(values[field]);
    if (!Number.isInteger(values.flags) || values.flags < 0 || values.flags > 7) throw new Error('Invalid search flags');
    bytes[offset++] = values.flags; u32(query.length); bytes.set(query, offset);
  }
  return bytes;
}
