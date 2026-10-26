import { AttachmentCheckStatus, AttachmentKey, MAX_ATTACHMENTS, MAX_READING_RESPONSE, RequestKind, ResponseKind } from './protocol_constants.mjs';

function flag(r) { const value = r.u8(); if (value > 1) throw new Error('Invalid document-service flag'); return value === 1; }
function optional(r, read) { return flag(r) ? read() : null; }
function blob(r) { return r.take(r.u32()); }
function key(r) { const value = r.u8(); if (!Object.values(AttachmentKey).includes(value)) throw new Error('Unknown attachment filename key'); return value; }
function list(r, minimum, cap, read) {
  const count = r.u32();
  if (count > cap || count > Math.floor((r.bytes.length - r.offset) / minimum)) throw new Error('Invalid document-service record count');
  return Array.from({ length: count }, read);
}

/** Protocol 1.8/1.10: strings are inert text, names are bytes, and sizes are exact BigInts. */
export function decodeDocumentService(kind, r) {
  if (r.bytes.length > MAX_READING_RESPONSE + 5) throw new Error('Document-service response budget exceeded');
  if (kind === ResponseKind.Metadata) {
    const info = optional(r, () => {
      const value = { object: optional(r, () => r.object()) };
      for (const field of ['title', 'author', 'subject', 'keywords', 'creator', 'producer', 'creationDate', 'modificationDate']) value[field] = optional(r, () => r.text());
      value.trapped = r.text();
      if (!['True', 'False', 'Unknown'].includes(value.trapped)) throw new Error('Invalid trapping declaration');
      return value;
    });
    return { info, language: optional(r, () => r.text()), metadataObject: optional(r, () => r.object()) };
  }
  if (kind === ResponseKind.MetadataBytes) return optional(r, () => blob(r));
  if (kind === ResponseKind.AttachmentBytes) return blob(r);
  if (kind === ResponseKind.AttachmentCheck) {
    const observation = () => {
      const value = r.u8();
      if (!Object.values(AttachmentCheckStatus).includes(value)) throw new Error('Unknown attachment check status');
      return value;
    };
    return { object: r.object(), decodedSize: r.number(8, 'getBigUint64'), size: observation(), checksum: observation() };
  }
  if (kind === ResponseKind.Attachments) return list(r, 18, MAX_ATTACHMENTS, (_, index) => {
    const value = { index, key: blob(r), object: optional(r, () => r.object()), names: { UF: optional(r, () => r.text()) } };
    for (const name of ['F', 'DOS', 'Mac', 'Unix']) value.names[name] = optional(r, () => blob(r));
    value.description = optional(r, () => r.text()); value.fileSystem = optional(r, () => blob(r));
    value.relationship = optional(r, () => blob(r)); value.volatile = flag(r);
    value.payloads = list(r, 9, 5, () => ({ key: key(r), object: r.object(), mediaType: optional(r, () => blob(r)),
      declaredSize: optional(r, () => r.number(8, 'getBigUint64')) }));
    if (new Set(value.payloads.map(payload => payload.key)).size !== value.payloads.length) throw new Error('Duplicate attachment payload key');
    return value;
  });
  throw new Error('Unknown document-service response');
}

export function encodeDocumentService(kind, values) {
  const selected = kind === RequestKind.AttachmentBytes || kind === RequestKind.AttachmentCheck;
  const bytes = new Uint8Array(selected ? 9 : 4), view = new DataView(bytes.buffer);
  const integer = (value, offset) => {
    if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) throw new Error('Expected a u32 document-service argument');
    view.setUint32(offset, value, true);
  };
  integer(values.document, 0);
  if (selected) {
    integer(values.index, 4);
    if (!Object.values(AttachmentKey).includes(values.key)) throw new Error('Unknown attachment filename key');
    bytes[8] = values.key;
  }
  return bytes;
}
