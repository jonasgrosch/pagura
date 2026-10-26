import { JobEventCode, MAX_FILE, MAX_FRAME, MAX_PASSWORD, MAX_READING_RESPONSE, OpenFlags, OutputCode,
  PngBackground, RequestKind, ResponseKind, ValidationCode } from './protocol_constants.mjs';

const u64Max = (1n << 64n) - 1n;
function wide(value) {
  if (typeof value === 'number' && !Number.isSafeInteger(value)) throw new Error('Expected an exact u64');
  if (typeof value !== 'number' && typeof value !== 'bigint') throw new Error('Expected a u64');
  const result = BigInt(value);
  if (result < 0n || result > u64Max) throw new Error('u64 outside range');
  return result;
}
function integer(value) {
  if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) throw new Error('Expected a u32');
  return value;
}
function bytes(value, maximum) {
  if (!(value instanceof Uint8Array) || value.length > maximum) throw new Error('Invalid progressive byte array');
  return value;
}
function version(value) {
  bytes(value, 32);
  if (value.length !== 32) throw new Error('Source version must contain 32 bytes');
  return value;
}
function code(value, table) {
  if (!Object.values(table).includes(value)) throw new Error('Unknown progressive code');
  return value;
}
function flag(r) { const value = r.u8(); if (value > 1) throw new Error('Invalid progressive flag'); return value === 1; }
function rectangle(r) {
  const rect = { x0: r.u32(), y0: r.u32(), x1: r.u32(), y1: r.u32() };
  if (rect.x0 >= rect.x1 || rect.y0 >= rect.y1) throw new Error('Invalid output rectangle');
  return rect;
}
function geometry(r) {
  const page = rectangle(r), region = rectangle(r);
  if (region.x0 < page.x0 || region.y0 < page.y0 || region.x1 > page.x1 || region.y1 > page.y1) throw new Error('Output region exceeds page');
  return { page, region };
}

/** Only V2 wire layouts. It receives the existing frame cursor and never reads PDF syntax. */
export function decodeProgressive(kind, r) {
  if (r.bytes.length > MAX_READING_RESPONSE + 5) throw new Error('Progressive response budget exceeded');
  switch (kind) {
    case ResponseKind.SourceCreate: case ResponseKind.SourceFinish: case ResponseKind.SourceClose: return { source: r.u32() };
    case ResponseKind.SourceSupply: return { added: r.u64(), generation: r.u64() };
    case ResponseKind.JobCreated: case ResponseKind.JobClose: return { job: r.u32() };
    case ResponseKind.JobAcknowledge: return { job: r.u32(), sequence: r.u64() };
    case ResponseKind.JobCancel: return { job: r.u32(), changed: flag(r) };
    case ResponseKind.JobStats: return { sources: r.u32(), jobs: r.u32(), sourceAdmission: r.u64(), activeAdmission: r.u64(), sourceResidency: r.u64() };
    case ResponseKind.NeedData: {
      const source = r.u64(), version = r.take(32), generation = r.u64(), count = r.u32();
      if (!source || !count || count > Math.floor((r.bytes.length - r.offset) / 16)) throw new Error('Invalid missing-range count');
      let previous = -1n;
      const ranges = Array.from({ length: count }, () => {
        const start = r.u64(), end = r.u64();
        if (start >= end || start < previous) throw new Error('Missing ranges must be sorted and disjoint');
        previous = end; return { start, end };
      });
      return { needData: true, source, version, generation, ranges };
    }
    case ResponseKind.JobProgress: return progress(r);
    default: throw new Error('Unknown progressive response');
  }
}

function progress(r) {
  const job = r.u32(), event = code(r.u8(), JobEventCode);
  if (!job) throw new Error('Invalid job handle');
  const value = { job, event };
  switch (event) {
    case JobEventCode.Opened: return { ...value, document: r.u32(), validation: code(r.u8(), ValidationCode) };
    case JobEventCode.Page: {
      const ordinal = r.u32(), reportedCount = flag(r) ? r.u32() : null, verifiedCount = flag(r) ? r.u32() : null;
      return { ...value, ordinal, reportedCount, verifiedCount, ...r.pageInfo() };
    }
    case JobEventCode.Prepared: return { ...value, output: code(r.u8(), OutputCode), ...geometry(r) };
    case JobEventCode.Band: {
      const sequence = r.u64(), bounds = geometry(r), firstRow = r.u32(), endRow = r.u32(), stride = r.u32();
      const length = r.u32(), size = (endRow - firstRow) * stride;
      if (!sequence || firstRow < bounds.region.y0 || endRow > bounds.region.y1 || firstRow >= endRow
        || stride !== (bounds.region.x1 - bounds.region.x0) * 4 || !Number.isSafeInteger(size) || size !== length) throw new Error('Invalid output band');
      return { ...value, sequence, ...bounds, firstRow, endRow, stride, premultipliedRgba: r.take(length) };
    }
    case JobEventCode.Png: {
      const sequence = r.u64(), length = r.u32();
      if (!sequence || !length) throw new Error('Invalid encoded output');
      return { ...value, sequence, bytes: r.take(length) };
    }
    default: return value;
  }
}

/** Bounded allocation and exact integer encoding; opaque versions remain byte arrays. */
export function encodeProgressive(kind, values) {
  let size;
  switch (kind) {
    case RequestKind.SourceCreate: size = 40; break;
    case RequestKind.SourceSupply: size = 48 + bytes(values.bytes, MAX_FILE).length; break;
    case RequestKind.OpenJob: size = 10 + bytes(values.password, MAX_PASSWORD).length; break;
    case RequestKind.PageJob: size = 8; break;
    case RequestKind.RenderJob: size = values.tile == null ? 19 : 35; break;
    case RequestKind.JobAcknowledge: size = 12; break;
    case RequestKind.JobStats: size = 0; break;
    case RequestKind.SourceFinish: case RequestKind.SourceClose: case RequestKind.JobAdvance: case RequestKind.JobCancel: case RequestKind.JobClose: size = 4; break;
    default: throw new Error('Unknown progressive request');
  }
  if (size > MAX_FRAME - 5) throw new Error('Progressive request budget exceeded');
  const result = new Uint8Array(size), view = new DataView(result.buffer); let offset = 0;
  const u32 = (value) => { view.setUint32(offset, integer(value), true); offset += 4; };
  const u64 = (value) => { view.setBigUint64(offset, wide(value), true); offset += 8; };
  const raw = (value) => { result.set(value, offset); offset += value.length; };
  const blob = (value) => { u32(value.length); raw(value); };
  switch (kind) {
    case RequestKind.SourceCreate: u64(values.length); raw(version(values.version)); break;
    case RequestKind.SourceSupply: u32(values.source); raw(version(values.version)); u64(values.offset); blob(values.bytes); break;
    case RequestKind.SourceFinish: case RequestKind.SourceClose: u32(values.source); break;
    case RequestKind.OpenJob:
      u32(values.source); result[offset++] = code(values.validation, ValidationCode);
      if (![0, OpenFlags.REPAIR].includes(values.recovery)) throw new Error('Invalid recovery mode');
      result[offset++] = values.recovery; blob(values.password); break;
    case RequestKind.PageJob: u32(values.document); u32(values.page); break;
    case RequestKind.RenderJob:
      u32(values.document); u32(values.page);
      if (!Number.isFinite(values.scale) || values.scale <= 0) throw new Error('Invalid scale');
      view.setFloat64(offset, values.scale, true); offset += 8;
      result[offset++] = code(values.background, PngBackground); result[offset++] = code(values.output, OutputCode);
      result[offset++] = values.tile == null ? 0 : 1;
      if (values.tile != null) for (const key of ['x', 'y', 'width', 'height']) u32(values.tile[key]);
      break;
    case RequestKind.JobAcknowledge: u32(values.job); u64(values.sequence); break;
    case RequestKind.JobAdvance: case RequestKind.JobCancel: case RequestKind.JobClose: u32(values.job); break;
    case RequestKind.JobStats: break;
  }
  return result;
}
