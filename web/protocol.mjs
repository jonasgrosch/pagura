import { decodeReading, encodeReading } from './reading_protocol.mjs';
/** Project-owned protocol v1 codec. See crates/pagura-host/PROTOCOL.md. */
export const MAX_FRAME = 512 * 1024 * 1024 + 32 * 1024 * 1024 + 14;
export const MAX_FILE = 512 * 1024 * 1024;
export const VERSION_1_1 = 0x00010001;
export const VERSION_1_2 = 0x00010002;
const utf8 = new TextDecoder('utf-8', { fatal: true });

class Reader {
  constructor(bytes) { this.bytes = bytes; this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength); this.offset = 0; }
  take(size) {
    if (!Number.isSafeInteger(size) || size < 0 || size > this.bytes.length - this.offset) throw new Error('Truncated protocol response');
    const part = this.bytes.subarray(this.offset, this.offset + size); this.offset += size; return part;
  }
  number(size, method) { const offset = this.offset; this.take(size); return this.view[method](offset, true); }
  u8() { return this.number(1, 'getUint8'); }
  u16() { return this.number(2, 'getUint16'); }
  u32() { return this.number(4, 'getUint32'); }
  f64() { return this.number(8, 'getFloat64'); }
  offset64() {
    const value = this.number(8, 'getBigUint64');
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error('Position exceeds JavaScript integer range');
    return Number(value);
  }
  text() { return utf8.decode(this.take(this.u32())); }
  object() { return { number: this.u32(), generation: this.u16() }; }
  context() {
    const present = this.u8();
    if (present > 1) throw new Error('Invalid object context flag');
    const object = present ? this.object() : null;
    const kind = this.u8();
    if (kind > 3) throw new Error('Unknown position kind');
    const position = { kind };
    if (kind) position.offset = this.offset64();
    if (kind === 2) position.object = this.object();
    return { object, position };
  }
}

/** A structured host failure, including feature, refused repair and source location. */
export class HostError extends Error {
  constructor(category, tag, repair, message, context) {
    super(message); this.name = 'HostError'; this.category = category; this.tag = tag; this.repair = repair;
    Object.assign(this, context);
  }
  toString() {
    const categories = ['', 'Syntax', 'Limit', 'Unsupported', 'MissingObject', 'Cycle', 'Password', 'InvalidArgument'];
    const object = this.object ? ` (object ${this.object.number} ${this.object.generation})` : '';
    const position = this.position?.kind ? ` at ${['', 'file', 'stream', 'buffer'][this.position.kind]} byte ${this.position.offset}` : '';
    const refused = this.repair ? `; recovery would repair it [${this.repair}]` : '';
    return `${categories[this.category]}${this.tag ? ` [${this.tag}]` : ''}: ${this.message}${object}${position}${refused}`;
  }
}

/** Decode exactly one bounded response body, preserving typed errors. */
export function decodeResponse(body) {
  if (body.length < 5 || body.length > MAX_FRAME) throw new Error('Invalid response frame length');
  const r = new Reader(body);
  const kind = r.u8(), id = r.u32();
  let value;
  switch (kind) {
    case 129: value = { version: r.u32() }; break;
    case 130: value = { document: r.u32(), pages: r.u32() }; break;
    case 131: {
      const crop = Array.from({ length: 4 }, () => r.f64());
      const rotation = r.u16(), userUnit = r.f64(), width = r.u32(), height = r.u32();
      if (!crop.every(Number.isFinite) || ![0, 90, 180, 270].includes(rotation)
        || !Number.isFinite(userUnit) || userUnit <= 0 || !width || !height) throw new Error('Invalid page geometry');
      value = { crop, rotation, userUnit, width, height }; break;
    }
    case 132: {
      const width = r.u32(), height = r.u32(), size = width * height * 4;
      if (!width || !height || !Number.isSafeInteger(size)) throw new Error('Invalid tile dimensions');
      const rgba = r.take(size); value = { width, height, rgba }; break;
    }
    case 133: {
      const count = r.u32();
      // Each record needs at least two lengths and two context bytes.
      if (count > Math.floor((body.length - r.offset) / 10)) throw new Error('Invalid diagnostic count');
      value = Array.from({ length: count }, () => ({ tag: r.text(), message: r.text(), ...r.context() })); break;
    }
    case 134: value = null; break;
    case 135: value = { generation: r.u32() }; break;
    case 136: {
      const status = r.u8(), generation = r.u32();
      if (status === 0) value = { cancelled: true, generation };
      else if (status === 1) {
        const width = r.u32(), height = r.u32(), size = width * height * 4;
        if (!width || !height || !Number.isSafeInteger(size)) throw new Error('Invalid tile dimensions');
        value = { width, height, rgba: r.take(size), generation, cancelled: false };
      } else throw new Error('Invalid generated tile status');
      break;
    }
    case 137: value = { sourceBytes: r.offset64(), cacheBytes: r.offset64(), documents: r.u32(), lists: r.u32() }; break;
    case 138: case 139: case 140: case 141: case 142: case 143: value = decodeReading(kind, r); break;
    case 255: {
      const category = r.u8(), tag = r.text(), repair = r.text(), message = r.text();
      if (category < 1 || category > 7 || (category === 3) !== (tag.length > 0) || (repair.length > 0 && category !== 1)) throw new Error('Invalid structured error');
      value = new HostError(category, tag, repair, message, r.context()); break;
    }
    default: throw new Error(`Unknown response kind ${kind}`);
  }
  if (r.offset !== body.length) throw new Error('Trailing protocol response bytes');
  return { kind, id, value };
}

/** Incremental framing; each response is allocated once, even across many writes. */
export class FrameDecoder {
  constructor(onFrame) { this.onFrame = onFrame; this.header = new Uint8Array(4); this.used = 0; this.body = null; }
  feed(bytes) {
    let offset = 0;
    while (offset < bytes.length) {
      const target = this.body ?? this.header;
      const count = Math.min(target.length - this.used, bytes.length - offset);
      target.set(bytes.subarray(offset, offset + count), this.used); offset += count; this.used += count;
      if (this.used !== target.length) continue;
      this.used = 0;
      if (!this.body) {
        const size = new DataView(this.header.buffer).getUint32(0, true);
        if (size < 5 || size > MAX_FRAME) throw new Error('Invalid response frame length');
        this.body = new Uint8Array(size);
      } else {
        const body = this.body; this.body = null; this.onFrame(decodeResponse(body));
      }
    }
  }
  finish() { if (this.used || this.body) throw new Error('Truncated response at host exit'); }
}

/** Encode canonical v1 and additive v1.1 requests. All numeric arguments are checked. */
export function encodeRequest(kind, id, values = {}) {
  const u32 = (value) => {
    if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) throw new Error('Expected a u32 argument');
    return value;
  };
  if (!u32(id)) throw new Error('Request identifier must be nonzero');
  if (kind >= 10 && kind <= 15) {
    const payload = encodeReading(kind, values), bytes = new Uint8Array(payload.length + 9), view = new DataView(bytes.buffer);
    view.setUint32(0, payload.length + 5, true); bytes[4] = kind; view.setUint32(5, id, true); bytes.set(payload, 9); return bytes;
  }
  let size;
  switch (kind) {
    case 1: size = 9; break;
    case 9: size = 5; break;
    case 2:
      if (!(values.bytes instanceof Uint8Array) || !(values.password instanceof Uint8Array)) throw new Error('Open requires byte arrays');
      if (values.bytes.length > MAX_FILE || values.password.length > 32 * 1024 * 1024) throw new Error('Open byte budget exceeded');
      if (![0, 1].includes(values.recovery)) throw new Error('Invalid recovery mode');
      size = 14 + values.password.length + values.bytes.length; break;
    case 3: case 7: size = 13; break;
    case 4: case 8:
      if (!Number.isFinite(values.scale) || values.scale <= 0) throw new Error('Scale must be positive and finite');
      size = kind === 8 ? 41 : 37; break;
    case 5: case 6: size = 9; break;
    default: throw new Error('Unknown request kind');
  }
  if (size > MAX_FRAME) throw new Error('Request frame budget exceeded');
  const bytes = new Uint8Array(size + 4), view = new DataView(bytes.buffer);
  view.setUint32(0, size, true); bytes[4] = kind; view.setUint32(5, id, true);
  let offset = 9;
  const integer = (value) => { view.setUint32(offset, u32(value), true); offset += 4; };
  const blob = (data) => { integer(data.length); bytes.set(data, offset); offset += data.length; };
  if (kind === 1) integer(values.version ?? 1);
  if (kind === 2) { bytes[offset++] = values.recovery; blob(values.password); blob(values.bytes); }
  if (kind === 8) integer(values.generation);
  if (kind >= 3 && kind <= 8) integer(values.document);
  if (kind === 7) integer(values.generation);
  if (kind === 3 || kind === 4 || kind === 8) integer(values.page);
  if (kind === 4 || kind === 8) {
    view.setFloat64(offset, values.scale, true); offset += 8;
    for (const key of ['x', 'y', 'width', 'height']) integer(values[key]);
  }
  return bytes;
}
