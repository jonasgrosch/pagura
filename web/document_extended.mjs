import { AttachmentKey, FileScopeCode, ImageAssetCode, ImageTransferCode, MAX_ATTACHMENTS, MAX_PASSWORD, MAX_READING_RESPONSE, MetadataOwnerCode, PayloadKind, RequestKind, ResponseKind } from './protocol_constants.mjs';

const names = { F: AttachmentKey.File, UF: AttachmentKey.Unicode, DOS: AttachmentKey.Dos, Mac: AttachmentKey.Mac, Unix: AttachmentKey.Unix };
function flag(r) { const value = r.u8(); if (value > 1) throw new Error('Invalid service flag'); return value === 1; }
function optional(r, read) { return flag(r) ? read() : null; }
function blob(r) { return r.take(r.u32()); }
function list(r, minimum, read) {
  const count = r.u32();
  if (count > MAX_ATTACHMENTS || count > Math.floor((r.bytes.length - r.offset) / minimum)) throw new Error('Invalid extended-service count');
  return Array.from({ length: count }, read);
}
function image(r) {
  const value = { object: r.object(), width: r.u32(), height: r.u32(), thumbnail: flag(r) };
  if (!value.width || !value.height) throw new Error('Invalid source image dimensions'); return value;
}
function parameters(r) {
  return optional(r, () => ({ size: optional(r, () => r.number(8, 'getBigUint64')), creationDate: optional(r, () => r.text()),
    modificationDate: optional(r, () => r.text()), checksum: optional(r, () => blob(r)) }));
}
function fileDetails(r, file) {
  file.identifiers = optional(r, () => [blob(r), blob(r)]);
  const payloads = list(r, 1, () => parameters(r));
  if (payloads.length !== file.payloads.length) throw new Error('Mismatched file parameter count');
  file.payloads.forEach((payload, index) => { payload.parameters = payloads[index]; });
  const indexes = new Map();
  file.related = list(r, 19, () => {
    const keyBytes = blob(r), name = blob(r), key = names[new TextDecoder('ascii', { fatal: true }).decode(keyBytes)];
    if (key === undefined) throw new Error('Invalid related-file key');
    const index = indexes.get(key) ?? 0; indexes.set(key, index + 1);
    return { key, index, name, object: r.object(), mediaType: optional(r, () => blob(r)), parameters: parameters(r) };
  });
  file.collectionItem = list(r, 9, () => {
    const key = blob(r), prefix = optional(r, () => r.text()), type = r.text(); let value;
    if (type === 'absent') value = null;
    else if (type === 'text') value = r.text();
    else if (type === 'integer') { const text = r.text(); if (!/^-?\d+$/.test(text)) throw new Error('Invalid collection integer'); value = BigInt(text); }
    else if (type === 'real') { value = r.f64(); if (!Number.isFinite(value)) throw new Error('Invalid collection real'); }
    else throw new Error('Unknown collection value type');
    return { key, prefix, type, value };
  });
  file.encryptedPayload = optional(r, () => ({ subtype: blob(r), version: optional(r, () => blob(r)) }));
  return file;
}

export function decodeExtended(kind, r, specification) {
  if (r.bytes.length > MAX_READING_RESPONSE + 5) throw new Error('Extended-service response budget exceeded');
  if (kind === ResponseKind.Reauthenticate) return { document: r.u32(), pages: r.u32() };
  if (kind === ResponseKind.Files) return list(r, 23, () => {
    const index = r.u32(), key = optional(r, () => blob(r)); return { index, key, ...fileDetails(r, specification(r)) };
  });
  if (kind === ResponseKind.ObjectMetadata) return optional(r, () => r.object());
  if (kind === ResponseKind.ObjectMetadataBytes) return optional(r, () => blob(r));
  if (kind === ResponseKind.Images) return list(r, 16, () => { const name = optional(r, () => blob(r)); return { name, ...image(r) }; });
  if (kind === ResponseKind.Thumbnail) return optional(r, () => image(r));
  if (kind === ResponseKind.ImageBytes) {
    const mode = r.text(), codec = optional(r, () => r.text());
    if (!['stored', 'encoded'].includes(mode) || (codec !== null && !['DCTDecode', 'JPXDecode', 'JBIG2Decode', 'CCITTFaxDecode'].includes(codec))) throw new Error('Unknown image transfer metadata');
    return { mode, codec, bytes: blob(r) };
  }
  if (kind === ResponseKind.Collection) return optional(r, () => {
    const object = optional(r, () => r.object()), initialFile = optional(r, () => blob(r)), view = r.text();
    if (!['Details', 'Tiles', 'Hidden', 'Navigator'].includes(view)) throw new Error('Invalid collection view');
    const fields = list(r, 19, () => ({ key: blob(r), name: r.text(), kind: r.text(), order: optional(r, () => BigInt(r.text())), visible: flag(r), editable: flag(r) }));
    const sort = list(r, 5, () => ({ key: blob(r), ascending: flag(r) }));
    return { object, initialFile, view, fields, sort };
  });
  throw new Error('Unknown extended-service response');
}

export function encodeExtended(kind, values) {
  const chunks = []; let length = 0;
  const push = bytes => { length += bytes.length; chunks.push(bytes); };
  const integer = (value, size = 4) => {
    if (!Number.isInteger(value) || value < 0 || value >= 2 ** (size * 8)) throw new Error('Invalid service integer');
    const bytes = new Uint8Array(size), view = new DataView(bytes.buffer);
    if (size === 1) view.setUint8(0, value); else if (size === 2) view.setUint16(0, value, true); else view.setUint32(0, value, true); push(bytes);
  };
  const code = (value, codes) => { if (!Object.values(codes).includes(value)) throw new Error('Unknown service selector'); integer(value, 1); };
  const object = value => { integer(value?.number); integer(value?.generation, 2); };
  const owner = value => { code(value?.kind, MetadataOwnerCode); if (value.kind === MetadataOwnerCode.Page) integer(value.page); else if (value.kind === MetadataOwnerCode.Object) object(value.object); };
  const scope = value => { code(value?.kind, FileScopeCode); if ([FileScopeCode.PageAssociated, FileScopeCode.PageAnnotations].includes(value.kind)) integer(value.page); else if (value.kind === FileScopeCode.ObjectAssociated) object(value.object); };
  integer(values.document);
  if (kind === RequestKind.Reauthenticate) {
    if (!(values.password instanceof Uint8Array) || values.password.length > MAX_PASSWORD) throw new Error('Invalid authentication password bytes');
    integer(values.password.length); push(values.password);
  } else if (kind === RequestKind.Files) scope(values.scope);
  else if ([RequestKind.FileBytes, RequestKind.FileCheck].includes(kind)) {
    scope(values.scope); integer(values.index); code(values.key, AttachmentKey);
    code(values.related === null || values.related === undefined ? PayloadKind.Embedded : PayloadKind.Related, PayloadKind);
    if (values.related !== null && values.related !== undefined) integer(values.related);
  } else if ([RequestKind.ObjectMetadata, RequestKind.ObjectMetadataBytes, RequestKind.Images, RequestKind.Thumbnail].includes(kind)) owner(values.owner);
  else if (kind === RequestKind.ImageBytes) { object(values.object); code(values.thumbnail ? ImageAssetCode.Thumbnail : ImageAssetCode.Image, ImageAssetCode); code(values.mode, ImageTransferCode); }
  else if (kind !== RequestKind.Collection) throw new Error('Unknown extended-service request');
  const bytes = new Uint8Array(length); let offset = 0; for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; } return bytes;
}
