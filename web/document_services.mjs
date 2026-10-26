import { AttachmentCheckStatus, AttachmentKey } from './protocol_constants.mjs';

const keyNames = { [AttachmentKey.File]: 'F', [AttachmentKey.Unicode]: 'UF', [AttachmentKey.Dos]: 'DOS', [AttachmentKey.Mac]: 'Mac', [AttachmentKey.Unix]: 'Unix' };
function byteName(bytes) {
  if (bytes === null) return null;
  return bytes.every(byte => byte >= 32 && byte <= 126) ? String.fromCharCode(...bytes.subarray(0, 1024))
    + (bytes.length > 1024 ? '…' : '') : 'Bytes: ' + [...bytes.subarray(0, 128)].map(byte => byte.toString(16).padStart(2, '0')).join('') + (bytes.length > 128 ? '…' : '');
}

/** Explicit byte downloads only. Source filenames, MIME types and URLs are inert. */
export function downloadDocumentBytes(bytes, name, document = globalThis.document, type = 'application/octet-stream') {
  const url = URL.createObjectURL(new Blob([bytes], { type }));
  try {
    const link = document.createElement('a'); link.href = url; link.download = name;
    document.body.append(link); try { link.click(); } finally { link.remove(); }
  } finally { setTimeout(() => URL.revokeObjectURL(url), 1000); }
}

/** A properties dialog owns no PDF bytes; stale requests cannot alter or download a newer document. */
export class DocumentServices {
  constructor({ state, document = globalThis.document, download = downloadDocumentBytes }) {
    Object.assign(this, { state, document, download }); this.serial = 0; this.buttons = [];
    const $ = id => document.getElementById(id);
    this.dialog = $('document-services-dialog'); this.openButton = $('document-services-open');
    this.info = $('document-properties'); this.files = $('document-attachments'); this.message = $('document-services-status');
    this.openButton.addEventListener('click', () => { void this.show(); });
    $('document-services-close').addEventListener('click', () => { this.reset(); this.openButton.focus(); });
    this.dialog.addEventListener('close', () => { this.serial++; });
  }
  controls() { this.openButton.disabled = this.state().handle === null; }
  reset() {
    this.serial++; this.buttons = []; this.info.replaceChildren(); this.files.replaceChildren(); this.message.textContent = '';
    if (this.dialog.open) this.dialog.close(); this.controls();
  }
  current(snapshot, serial) {
    const now = this.state();
    return this.serial === serial && this.dialog.open && snapshot.epoch === now.epoch && snapshot.client === now.client && snapshot.handle === now.handle;
  }
  node(tag, text) { const node = this.document.createElement(tag); node.textContent = text; return node; }
  async show() {
    const snapshot = { ...this.state() }; if (snapshot.handle === null) return;
    const serial = ++this.serial; this.buttons = []; this.message.textContent = '';
    this.info.replaceChildren(this.node('p', 'Loading properties…')); this.files.replaceChildren(this.node('p', 'Loading attachments…'));
    if (!this.dialog.open) this.dialog.showModal();
    const results = await Promise.allSettled([snapshot.client.metadata(snapshot.handle), snapshot.client.attachments(snapshot.handle)]);
    if (!this.current(snapshot, serial)) return;
    this.info.replaceChildren(); this.files.replaceChildren();
    if (results[0].status === 'fulfilled') this.properties(results[0].value, snapshot, serial);
    else this.info.append(this.node('p', `Properties unavailable: ${results[0].reason}`));
    if (results[1].status === 'fulfilled') this.attachments(results[1].value, snapshot, serial);
    else this.files.append(this.node('p', `Attachments unavailable: ${results[1].reason}`));
  }
  properties(value, snapshot, serial) {
    const list = this.document.createElement('dl');
    const row = (label, text) => { if (text !== null && text !== undefined) list.append(this.node('dt', label), this.node('dd', text)); };
    for (const [key, label] of [['title', 'Title'], ['author', 'Author'], ['subject', 'Subject'], ['keywords', 'Keywords'], ['creator', 'Creator'], ['producer', 'Producer'], ['creationDate', 'Created (declared)'], ['modificationDate', 'Modified (declared)'], ['trapped', 'Trapping (declared)']]) row(label, value.info?.[key]);
    row('Language (declared)', value.language); this.info.append(list);
    if (!value.info && value.language === null) this.info.append(this.node('p', 'No document properties declared.'));
    if (value.metadataObject) this.info.append(this.button('Download metadata packet', snapshot, serial,
      () => snapshot.client.metadataBytes(snapshot.handle), 'metadata.xmp'));
  }
  attachments(files, snapshot, serial) {
    if (!files.length) { this.files.append(this.node('p', 'No catalog attachments.')); return; }
    const list = this.document.createElement('ol'); this.files.append(list);
    for (const file of files) {
      const item = this.document.createElement('li');
      item.append(this.node('p', file.names.UF ?? byteName(file.names.F) ?? `Attachment ${file.index + 1}`));
      if (file.description !== null) item.append(this.node('p', file.description));
      for (const payload of file.payloads) {
        const name = keyNames[payload.key], label = file.payloads.length === 1 ? 'Download attachment' : `Download ${name} payload`;
        const group = this.document.createElement('div');
        group.append(this.button(label, snapshot, serial, () => snapshot.client.attachmentBytes(snapshot.handle, file.index, payload.key), `attachment-${file.index + 1}-${name}.bin`));
        if (payload.declaredSize !== null) group.append(this.node('span', ` ${payload.declaredSize} bytes (declared)`));
        const result = this.node('p', 'Size and checksum have not been checked.');
        const check = this.node('button', file.payloads.length === 1 ? 'Check size and checksum' : `Check ${name} size and checksum`);
        check.type = 'button'; this.buttons.push(check);
        check.addEventListener('click', () => { void this.check(snapshot, serial, file.index, payload.key, result); });
        group.append(check, result);
        item.append(group);
      }
      if (!file.payloads.length) item.append(this.node('p', 'No embedded payload.'));
      list.append(item);
    }
  }
  button(label, snapshot, serial, read, name) {
    const button = this.node('button', label); button.type = 'button'; this.buttons.push(button);
    button.addEventListener('click', () => { void this.extract(snapshot, serial, read, name); }); return button;
  }
  async extract(snapshot, serial, read, name) {
    if (!this.current(snapshot, serial) || this.buttons.some(button => button.disabled)) return;
    this.buttons.forEach(button => { button.disabled = true; }); this.message.textContent = 'Preparing download…';
    try {
      const bytes = await read(); if (!this.current(snapshot, serial)) return;
      if (bytes === null) throw new Error('No metadata packet is present');
      this.download(bytes, name, this.document); this.message.textContent = 'Download started.';
    } catch (error) { if (this.current(snapshot, serial)) this.message.textContent = `Download unavailable: ${error}`; }
    finally { if (this.current(snapshot, serial)) this.buttons.forEach(button => { button.disabled = false; }); }
  }
  async check(snapshot, serial, index, key, result) {
    if (!this.current(snapshot, serial) || this.buttons.some(button => button.disabled)) return;
    this.buttons.forEach(button => { button.disabled = true; });
    this.message.textContent = 'Checking attachment…'; result.textContent = 'Checking size and checksum…';
    try {
      const checked = await snapshot.client.attachmentCheck(snapshot.handle, index, key);
      if (!this.current(snapshot, serial)) return;
      const label = value => {
        if (value === AttachmentCheckStatus.NotDeclared) return 'not declared';
        if (value === AttachmentCheckStatus.Matches) return 'matches';
        if (value === AttachmentCheckStatus.Mismatch) return 'does not match';
        throw new Error('Unknown attachment check result');
      };
      result.textContent = `${checked.decodedSize} decoded bytes. Declared size: ${label(checked.size)}. Declared checksum: ${label(checked.checksum)}. This checks declarations, not file safety or authenticity.`;
      this.message.textContent = checked.size === AttachmentCheckStatus.Mismatch || checked.checksum === AttachmentCheckStatus.Mismatch
        ? 'Attachment declarations do not match its decoded bytes.' : 'Attachment check completed.';
    } catch (error) {
      if (this.current(snapshot, serial)) { result.textContent = `Check unavailable: ${error}`; this.message.textContent = 'Attachment check failed.'; }
    } finally { if (this.current(snapshot, serial)) this.buttons.forEach(button => { button.disabled = false; }); }
  }
}
