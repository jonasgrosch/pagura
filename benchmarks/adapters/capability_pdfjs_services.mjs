// pdf.js 6.4 public attachment API: metadata Map plus lazy byte retrieval.
import { createHash } from 'node:crypto';

export class ProbeLimit extends Error {}

function attachmentValue(item, content) {
  if (typeof item?.filename !== 'string' || !(content instanceof Uint8Array)) {
    throw new TypeError('missing attachment bytes or filename');
  }
  if (content.length > 64 * 1024 * 1024) throw new ProbeLimit('attachment exceeds 64 MiB adapter bound');
  return { name: item.filename, sha256: createHash('sha256').update(content).digest('hex') };
}

function sorted(values) {
  return values.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
}

export async function attachmentValues(document) {
  const attachments = await document.getAttachments();
  if (attachments === null) return [];
  if (!(attachments instanceof Map)) throw new TypeError('expected attachment Map');
  if (attachments.size > 1024) throw new ProbeLimit('attachment count exceeds adapter bound');
  const values = [];
  for (const [id, item] of attachments) {
    if (typeof id !== 'string' || typeof item?.filename !== 'string') {
      throw new TypeError('invalid attachment descriptor');
    }
    const content = item.content ?? await document.getAttachmentContent(id);
    values.push(attachmentValue(item, content));
  }
  return sorted(values);
}

export async function annotationAttachmentValues(document, fileAttachmentType) {
  const page = await document.getPage(1);
  const annotations = await page.getAnnotations({ intent: 'any' });
  if (!Array.isArray(annotations)) throw new TypeError('expected annotation array');
  if (annotations.length > 1024) throw new ProbeLimit('annotation count exceeds adapter bound');
  const values = [];
  for (const item of annotations) {
    if (item.annotationType !== fileAttachmentType) continue;
    if (typeof item.fileId !== 'string' || typeof item.file?.filename !== 'string') {
      throw new TypeError('invalid file annotation descriptor');
    }
    const content = await document.getAttachmentContent(item.fileId);
    values.push(attachmentValue(item.file, content));
  }
  return sorted(values);
}
