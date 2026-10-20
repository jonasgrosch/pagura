/** Protocol 1.5: bounded field descriptors and atomic change batches. */
const MAX_FIELDS = 1024, MAX_OPTIONS = 4096, MAX_BYTES = 32 * 1024 * 1024;
function count(r, cap, minimum) {
  const value = r.u32();
  if (value > cap || value > Math.floor((r.bytes.length - r.offset) / minimum)) throw new Error('Invalid form record count');
  return value;
}
export function decodeFields(r) {
  if (r.bytes.length > MAX_BYTES + 5) throw new Error('Form metadata exceeds budget');
  let total = 0;
  return Array.from({ length: count(r, MAX_FIELDS, 18) }, () => {
    const index = r.u32(), kind = r.u8(), flags = r.u8(), name = r.text(), value = r.text();
    if (kind > 3 || flags > 15) throw new Error('Invalid form descriptor');
    const length = count(r, MAX_OPTIONS - total, 13); total += length;
    const options = Array.from({ length }, () => {
      const key = r.u32(), label = r.text(), selected = r.u8(), state = r.take(r.u32());
      if (selected > 1) throw new Error('Invalid form selection');
      return { key, label, selected: Boolean(selected), state };
    });
    return { index, kind, flags, name, value, options };
  });
}
export function encodeChanges(changes) {
  if (!Array.isArray(changes) || changes.length > MAX_FIELDS) throw new Error('Too many form changes');
  const chunks = []; let size = 0, previous = -1;
  const put = (bytes) => { size += bytes.length; if (size > MAX_BYTES) throw new Error('Form changes exceed byte budget'); chunks.push(bytes); };
  const u32 = (value) => {
    if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) throw new Error('Invalid form index');
    const bytes = new Uint8Array(4); new DataView(bytes.buffer).setUint32(0, value, true); put(bytes);
  };
  const blob = (bytes) => { if (!(bytes instanceof Uint8Array)) throw new Error('Invalid form bytes'); u32(bytes.length); put(bytes); };
  const text = (value) => {
    if (typeof value !== 'string' || value.length > MAX_BYTES / 4) throw new Error('Form text exceeds budget');
    blob(new TextEncoder().encode(value));
  };
  u32(changes.length);
  for (const change of changes) {
    if (change.index <= previous || ![0, 1, 2, 3, 4].includes(change.kind)) throw new Error('Invalid or duplicate form change');
    previous = change.index; u32(change.index); put(Uint8Array.of(change.kind));
    if (change.kind === 0 || change.kind === 4) text(change.value);
    else if (change.kind === 1) {
      if (!Array.isArray(change.indices) || change.indices.length > MAX_OPTIONS) throw new Error('Too many choice indexes');
      u32(change.indices.length); for (const index of change.indices) u32(index);
    } else if (change.kind === 3) { u32(change.widget); blob(change.state); }
  }
  const result = new Uint8Array(size); let offset = 0;
  for (const bytes of chunks) { result.set(bytes, offset); offset += bytes.length; }
  return result;
}

export function decodeWidgets(r) {
  return Array.from({ length: count(r, MAX_OPTIONS, 41) }, () => {
    const index = r.u32(), field = r.u32(), flags = r.u8();
    const rect = Array.from({ length: 4 }, () => r.f64());
    if (flags > 15 || rect.some((n) => !Number.isFinite(n)) || rect[2] <= rect[0] || rect[3] <= rect[1]) throw new Error('Invalid form widget geometry');
    return { index, field, flags, rect };
  });
}
