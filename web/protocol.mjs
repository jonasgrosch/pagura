import { decodeFields, decodeWidgets, decodeStyles, encodeChanges } from './editing_protocol.mjs';
import { decodeReading, encodeReading } from './reading_protocol.mjs';
import { decodeDocumentService, encodeDocumentService } from './document_protocol.mjs';
import { DiagnosticCode, ErrorCategory, MAX_FILE, MAX_FRAME, MAX_PASSWORD, OpenFlags, PositionKind, RequestKind, ResponseKind, TileStatus, Version } from './protocol_constants.mjs';
/** Project-owned protocol v1 codec. See crates/pagura-host/PROTOCOL.md. */
export { MAX_FILE, MAX_FRAME };
export const VERSION_1_1 = Version.V1_1;
export const VERSION_1_2 = Version.V1_2;
export const VERSION_1_3 = Version.V1_3;
export const VERSION_1_4 = Version.V1_4;
export const VERSION_1_5 = Version.V1_5;
export const VERSION_1_6 = Version.V1_6;
export const VERSION_1_7 = Version.V1_7;
export const VERSION_1_8 = Version.V1_8;
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
    if (kind > PositionKind.Buffer) throw new Error('Unknown position kind');
    const position = { kind };
    if (kind !== PositionKind.Unknown) position.offset = this.offset64();
    if (kind === PositionKind.Stream) position.object = this.object();
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
    case ResponseKind.Hello: value = { version: r.u32() }; break;
    case ResponseKind.Open: value = { document: r.u32(), pages: r.u32() }; break;
    case ResponseKind.PageInfo: {
      const crop = Array.from({ length: 4 }, () => r.f64());
      const rotation = r.u16(), userUnit = r.f64(), width = r.u32(), height = r.u32();
      if (!crop.every(Number.isFinite) || ![0, 90, 180, 270].includes(rotation)
        || !Number.isFinite(userUnit) || userUnit <= 0 || !width || !height) throw new Error('Invalid page geometry');
      value = { crop, rotation, userUnit, width, height }; break;
    }
    case ResponseKind.Tile: {
      const width = r.u32(), height = r.u32(), size = width * height * 4;
      if (!width || !height || !Number.isSafeInteger(size)) throw new Error('Invalid tile dimensions');
      const rgba = r.take(size); value = { width, height, rgba }; break;
    }
    case ResponseKind.Diagnostics: case ResponseKind.DiagnosticDetails: {
      const count = r.u32();
      // Two lengths, context flags, and a kind byte in typed snapshots.
      if (count > Math.floor((body.length - r.offset) / (kind === ResponseKind.DiagnosticDetails ? 11 : 10))) throw new Error('Invalid diagnostic count');
      value = Array.from({ length: count }, () => {
        const diagnosticKind = kind === ResponseKind.DiagnosticDetails ? r.u8() : DiagnosticCode.Repair;
        if (diagnosticKind > DiagnosticCode.Information) throw new Error('Unknown diagnostic kind');
        return { kind: diagnosticKind === DiagnosticCode.Repair ? 'repair' : 'information', tag: r.text(), message: r.text(), ...r.context() };
      }); break;
    }
    case ResponseKind.Close: value = null; break;
    case ResponseKind.Generation: value = { generation: r.u32() }; break;
    case ResponseKind.GenerationTile: {
      const status = r.u8(), generation = r.u32();
      if (status === TileStatus.Skipped) value = { cancelled: true, generation };
      else if (status === TileStatus.Rendered) {
        const width = r.u32(), height = r.u32(), size = width * height * 4;
        if (!width || !height || !Number.isSafeInteger(size)) throw new Error('Invalid tile dimensions');
        value = { width, height, rgba: r.take(size), generation, cancelled: false };
      } else throw new Error('Invalid generated tile status');
      break;
    }
    case ResponseKind.Stats: value = { sourceBytes: r.offset64(), cacheBytes: r.offset64(), documents: r.u32(), lists: r.u32() }; break;
    case ResponseKind.PageText: case ResponseKind.Search: case ResponseKind.Links: case ResponseKind.Outlines: case ResponseKind.Labels: case ResponseKind.ReadingStats: case ResponseKind.Bounds: value = decodeReading(kind, r); break;
    case ResponseKind.FormFields: value = decodeFields(r); break;
    case ResponseKind.Metadata: case ResponseKind.Attachments: case ResponseKind.MetadataBytes: case ResponseKind.AttachmentBytes: value = decodeDocumentService(kind, r); break;
    case ResponseKind.FormStyles: value = decodeStyles(r); break;
    case ResponseKind.FormWidgets: value = decodeWidgets(r); break;
    case ResponseKind.SavedForm: { const length = r.u32(); if (length > MAX_FILE) throw new Error('Saved PDF exceeds budget'); value = r.take(length); break; }
    case ResponseKind.Error: {
      const category = r.u8(), tag = r.text(), repair = r.text(), message = r.text();
      if (!Object.values(ErrorCategory).includes(category) || (category === ErrorCategory.Unsupported) !== (tag.length > 0)) throw new Error('Invalid structured error');
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

/** Encode canonical v1 and additive requests. All numeric arguments are checked. */
export function encodeRequest(kind, id, values = {}) {
  const u32 = (value) => {
    if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) throw new Error('Expected a u32 argument');
    return value;
  };
  if (!u32(id)) throw new Error('Request identifier must be nonzero');
  const reading = kind >= RequestKind.PageText && kind <= RequestKind.Bounds;
  if (reading || kind >= RequestKind.Metadata && kind <= RequestKind.AttachmentBytes) {
    const payload = reading ? encodeReading(kind, values) : encodeDocumentService(kind, values), bytes = new Uint8Array(payload.length + 9), view = new DataView(bytes.buffer);
    view.setUint32(0, payload.length + 5, true); bytes[4] = kind; view.setUint32(5, id, true); bytes.set(payload, 9); return bytes;
  }
  const changes = kind === RequestKind.SaveForm ? encodeChanges(values.changes) : null;
  let size;
  switch (kind) {
    case RequestKind.Hello: size = 9; break;
    case RequestKind.Stats: size = 5; break;
    case RequestKind.Open:
      if (!(values.bytes instanceof Uint8Array) || !(values.password instanceof Uint8Array)) throw new Error('Open requires byte arrays');
      if (values.bytes.length > MAX_FILE || values.password.length > MAX_PASSWORD) throw new Error('Open byte budget exceeded');
      if (![0, OpenFlags.REPAIR].includes(values.recovery)) throw new Error('Invalid recovery mode');
      size = 14 + values.password.length + values.bytes.length; break;
    case RequestKind.PageInfo: case RequestKind.Generation: case RequestKind.FormWidgets: case RequestKind.FormStyles: size = 13; break;
    case RequestKind.RenderTile: case RequestKind.GenerationTile:
      if (!Number.isFinite(values.scale) || values.scale <= 0) throw new Error('Scale must be positive and finite');
      size = kind === RequestKind.GenerationTile ? 41 : 37; break;
    case RequestKind.SaveForm: size = 13 + changes.length; break;
    case RequestKind.Diagnostics: case RequestKind.Close: case RequestKind.DiagnosticDetails: case RequestKind.FormFields: size = 9; break;
    default: throw new Error('Unknown request kind');
  }
  if (size > MAX_FRAME) throw new Error('Request frame budget exceeded');
  const bytes = new Uint8Array(size + 4), view = new DataView(bytes.buffer);
  view.setUint32(0, size, true); bytes[4] = kind; view.setUint32(5, id, true);
  let offset = 9;
  const integer = (value) => { view.setUint32(offset, u32(value), true); offset += 4; };
  const blob = (data) => { integer(data.length); bytes.set(data, offset); offset += data.length; };
  if (kind === RequestKind.Hello) integer(values.version ?? Version.V1);
  if (kind === RequestKind.Open) { bytes[offset++] = values.recovery; blob(values.password); blob(values.bytes); }
  if (kind === RequestKind.GenerationTile) integer(values.generation);
  if ((kind >= RequestKind.PageInfo && kind <= RequestKind.GenerationTile) || [RequestKind.DiagnosticDetails, RequestKind.FormFields, RequestKind.SaveForm, RequestKind.FormWidgets, RequestKind.FormStyles].includes(kind)) integer(values.document);
  if (kind === RequestKind.SaveForm) blob(changes);
  if (kind === RequestKind.Generation) integer(values.generation);
  if (kind === RequestKind.PageInfo || kind === RequestKind.RenderTile || kind === RequestKind.GenerationTile || kind === RequestKind.FormWidgets || kind === RequestKind.FormStyles) integer(values.page);
  if (kind === RequestKind.RenderTile || kind === RequestKind.GenerationTile) {
    view.setFloat64(offset, values.scale, true); offset += 8;
    for (const key of ['x', 'y', 'width', 'height']) integer(values[key]);
  }
  return bytes;
}
