// pdf.js 6.4 public attachment API: metadata Map plus lazy byte retrieval.
import { createHash } from 'node:crypto';

export async function attachmentValues(document) {
  const attachments = await document.getAttachments();
  if (attachments === null) return [];
  if (!(attachments instanceof Map)) throw new TypeError('expected attachment Map');
  const values = [];
  for (const [id, item] of attachments) {
    if (typeof id !== 'string' || typeof item?.filename !== 'string') {
      throw new TypeError('invalid attachment descriptor');
    }
    const content = item.content ?? await document.getAttachmentContent(id);
    if (!(content instanceof Uint8Array)) throw new TypeError('missing attachment bytes');
    values.push({ name: item.filename, sha256: createHash('sha256').update(content).digest('hex') });
  }
  return values.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
}
