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

function specification(r) {
  const value = { object: optional(r, () => r.object()), names: { UF: optional(r, () => r.text()) } };
  for (const name of ['F', 'DOS', 'Mac', 'Unix']) value.names[name] = optional(r, () => blob(r));
  value.description = optional(r, () => r.text()); value.fileSystem = optional(r, () => blob(r));
  value.relationship = optional(r, () => blob(r)); value.volatile = flag(r);
  value.payloads = list(r, 9, 5, () => ({ key: key(r), object: r.object(), mediaType: optional(r, () => blob(r)),
    declaredSize: optional(r, () => r.number(8, 'getBigUint64')) }));
  if (new Set(value.payloads.map(payload => payload.key)).size !== value.payloads.length) throw new Error('Duplicate attachment payload key');
  return value;
}

/** Protocol 1.8/1.10/1.11: strings are inert text, names are bytes, and sizes are exact BigInts. */
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
  if (kind === ResponseKind.AttachmentBytes || kind === ResponseKind.PageAttachmentBytes) return blob(r);
  if (kind === ResponseKind.AttachmentCheck || kind === ResponseKind.PageAttachmentCheck) {
    const observation = () => {
      const value = r.u8();
      if (!Object.values(AttachmentCheckStatus).includes(value)) throw new Error('Unknown attachment check status');
      return value;
    };
    return { object: r.object(), decodedSize: r.number(8, 'getBigUint64'), size: observation(), checksum: observation() };
  }
  if (kind === ResponseKind.Attachments) return list(r, 18, MAX_ATTACHMENTS, (_, index) => {
    const name = blob(r);
    return { index, key: name, ...specification(r) };
  });
  if (kind === ResponseKind.PageAttachments) {
    let previous = -1;
    return list(r, 58, MAX_ATTACHMENTS, () => {
      const index = r.u32(), object = r.object(), rectangle = Array.from({ length: 4 }, () => r.f64());
      if (index <= previous) throw new Error('Page attachment indexes must increase');
      previous = index;
      if (!rectangle.every(Number.isFinite) || rectangle[0] > rectangle[2] || rectangle[1] > rectangle[3]) {
        throw new Error('Invalid page attachment rectangle');
      }
      return { index, object, rectangle, contents: optional(r, () => r.text()),
        icon: optional(r, () => blob(r)), file: specification(r) };
    });
  }
  throw new Error('Unknown document-service response');
}

export function encodeDocumentService(kind, values) {
  const page = kind >= RequestKind.PageAttachments && kind <= RequestKind.PageAttachmentCheck;
  const selected = page ? kind !== RequestKind.PageAttachments : kind === RequestKind.AttachmentBytes || kind === RequestKind.AttachmentCheck;
  const indexOffset = page ? 8 : 4;
  const bytes = new Uint8Array(indexOffset + (selected ? 5 : 0)), view = new DataView(bytes.buffer);
  const integer = (value, offset) => {
    if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) throw new Error('Expected a u32 document-service argument');
    view.setUint32(offset, value, true);
  };
  integer(values.document, 0);
  if (page) integer(values.page, 4);
  if (selected) {
    integer(values.index, indexOffset);
    if (!Object.values(AttachmentKey).includes(values.key)) throw new Error('Unknown attachment filename key');
    bytes[indexOffset + 4] = values.key;
  }
  return bytes;
}
