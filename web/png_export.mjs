import { downloadDocumentBytes } from './document_services.mjs';
import { PngBackground } from './protocol_constants.mjs';

/** Export a click-time form snapshot without adopting it or changing drafts. */
export async function exportPagePng(snapshot, { page, scale, background }, editor, current) {
  if (editor.busy) throw new Error('Wait for the current PDF save to finish.');
  const changes = structuredClone([...editor.changes.values()].sort((a, b) => a.index - b.index));
  if (!changes.length) return snapshot.client.pagePng(snapshot.handle, page, scale, background);
  const editing = editor.editing;
  if (!editing || editing.client !== snapshot.client) throw new Error('Form values are not ready to export.');
  const bytes = await editing.client.saveForm(editing.handle, changes);
  if (!current()) return null;
  const opened = await snapshot.client.open(bytes, 1);
  try {
    if (!current()) return null;
    return await snapshot.client.pagePng(opened.document, page, scale, background);
  } finally { await snapshot.client.close(opened.document); }
}

/** Explicit page export; closing or changing the document discards pending bytes. */
export class PngExport {
  constructor({ state, editor, document = globalThis.document, download = downloadDocumentBytes }) {
    Object.assign(this, { state, editor, document, download }); this.serial = 0; this.busy = false;
    const $ = id => document.getElementById(id);
    this.dialog = $('png-export-dialog'); this.openButton = $('png-export-open'); this.form = $('png-export-form');
    this.page = $('png-export-page'); this.dpi = $('png-export-dpi'); this.transparent = $('png-export-transparent');
    this.submit = $('png-export-submit'); this.message = $('png-export-status');
    this.openButton.addEventListener('click', () => this.show());
    this.form.addEventListener('submit', event => { event.preventDefault(); void this.export(); });
    $('png-export-close').addEventListener('click', () => { this.reset(); this.openButton.focus(); });
    this.dialog.addEventListener('close', () => { this.serial++; });
  }
  controls() { this.openButton.disabled = this.state().handle === null || !this.state().count; }
  reset() { this.serial++; if (this.dialog.open) this.dialog.close(); this.message.textContent = ''; this.controls(); }
  show() {
    const snapshot = this.state(); if (snapshot.handle === null || !snapshot.count) return;
    this.serial++; this.page.value = String(snapshot.page + 1); this.page.max = String(snapshot.count);
    this.message.textContent = this.busy ? 'Finishing the previous export…' : ''; if (!this.dialog.open) this.dialog.showModal();
  }
  current(snapshot, serial) {
    const state = this.state();
    return this.serial === serial && this.dialog.open && snapshot.epoch === state.epoch
      && snapshot.client === state.client && snapshot.handle === state.handle;
  }
  async export() {
    if (this.busy || !this.dialog.open) return;
    const snapshot = { ...this.state() }, serial = this.serial;
    if (snapshot.handle === null) return;
    const page = Number(this.page.value) - 1, dpi = Number(this.dpi.value);
    if (!Number.isInteger(page) || page < 0 || page >= snapshot.count || !Number.isInteger(dpi) || dpi < 1 || dpi > 2400) {
      this.message.textContent = 'Choose a page in this document and a resolution from 1 to 2400 DPI.'; return;
    }
    const options = { page, scale: dpi / 72, background: this.transparent.checked ? PngBackground.Transparent : PngBackground.White };
    this.busy = true; this.submit.disabled = true; this.message.textContent = 'Preparing PNG…';
    try {
      const bytes = await exportPagePng(snapshot, options, this.editor, () => this.current(snapshot, serial));
      if (!this.current(snapshot, serial) || bytes === null) return;
      this.download(bytes, `page-${String(page + 1).padStart(4, '0')}.png`, this.document, 'image/png');
      this.message.textContent = 'PNG download started.';
    } catch (error) { if (this.current(snapshot, serial)) this.message.textContent = `PNG export unavailable: ${error}`; }
    finally { this.busy = false; this.submit.disabled = false; }
  }
}
