import { ButtonArtwork } from './form_buttons.mjs';
import { applyTextStyle } from './form_typography.mjs';
import { FormPreview } from './form_preview.mjs';
import { widgetControl, positionWidget } from './form_layer.mjs';

/** Drafts live independently of virtualized page controls and immutable host documents. */
export class FormEditor {
  constructor({ state, adopt, download, redraw = () => {}, document: doc = document, preview = true }) {
    Object.assign(this, { state, adopt, download, redraw, doc });
    this.message = doc.getElementById('form-status'); this.saveButton = doc.getElementById('form-download');
    this.openButton = doc.getElementById('edit-open'); this.resetButton = doc.getElementById('form-reset');
    this.changes = new Map(); this.pages = new Map(); this.fields = null; this.editing = null;
    this.buttons = preview ? new ButtonArtwork(this) : null;
    this.preview = preview ? new FormPreview(this) : null;
    this.token = 0; this.active = true; this.busy = false;
    this.openButton.addEventListener('click', () => {
      this.active = !this.active; this.controls();
      if (this.active && !this.fields) void this.open({ keepDraft: true });
      else { for (const record of this.pages.values()) record.layer.hidden = !this.active; this.redraw(); }
    });
    this.resetButton.addEventListener('click', () => { this.preview?.clear(); this.changes.clear(); this.sync(); this.message.textContent = 'Changes discarded.'; this.controls(); });
    this.saveButton.addEventListener('click', () => { void this.save(); });
  }
  controls() {
    const { handle } = this.state(); this.openButton.disabled = handle === null || this.busy;
    this.openButton.setAttribute('aria-pressed', String(this.active));
    this.saveButton.disabled = this.busy || handle === null;
    this.resetButton.disabled = this.busy || !this.changes.size;
    for (const record of this.pages.values()) record.layer.inert = this.busy;
  }
  prune(indexes) { for (const [index, record] of this.pages) if (!indexes.includes(index)) { record.layer.remove(); this.pages.delete(index); } }
  reset() {
    this.preview?.clear();
    ++this.token; this.prune([]); this.changes.clear(); this.fields = null; this.busy = false;
    const editing = this.editing; this.editing = null;
    if (editing?.owned) void editing.client.close(editing.handle).catch(() => {});
    this.message.textContent = ''; this.controls();
  }
  recovered() { void this.open({ keepDraft: true }); }
  async open({ keepDraft = false } = {}) {
    const changes = keepDraft ? new Map(this.changes) : new Map(); this.reset(); this.changes = changes;
    if (!this.active) return;
    const token = this.token, state = this.state();
    if (state.handle === null) return;
    this.message.textContent = 'Loading fillable fields…';
    try {
      if (state.recovery !== 0 && !(await state.client.formFields(state.handle)).length) {
        if (token === this.token) { this.fields = new Map(); this.message.textContent = ''; this.controls(); }
        return true;
      }
      if (token !== this.token) return;
      // Rendering may use explicit compatibility repairs. Editing metadata and
      // saving both use a separate strict snapshot of the exact original bytes.
      const opened = state.recovery === 0 ? { document: state.handle } : await state.client.open(state.bytes, 0, state.password);
      const editing = { client: state.client, handle: opened.document, owned: state.recovery !== 0 };
      if (token !== this.token) { if (editing.owned) await editing.client.close(editing.handle); return; }
      this.editing = editing;
      const fields = await editing.client.formFields(editing.handle);
      if (token !== this.token) return;
      this.fields = new Map(fields.map((field) => [field.index, field]));
      this.message.textContent = fields.length ? 'Fillable PDF' : '';
      this.controls(); this.redraw(); this.preview?.schedule(); return true;
    } catch (error) { if (token === this.token) this.message.textContent = `Form editing unavailable: ${error}`; }
    this.controls();
  }
  async page(index, info, parent, zoom) {
    if (!this.active || !this.fields?.size || !this.editing) return;
    let record = this.pages.get(index);
    if (record && record.parent !== parent) { record.layer.remove(); this.pages.delete(index); record = null; }
    if (!record) {
      const token = this.token, editing = this.editing;
      const layer = this.doc.createElement('div'); layer.className = 'form-layer';
      record = { layer, parent, controls: [] }; this.pages.set(index, record); parent.append(layer);
      try {
        const widgets = await editing.client.formWidgets(editing.handle, index);
        const view = this.state();
        const styles = new Map((await view.client.formStyles?.(view.handle, index) ?? []).map(style => [style.index, style]));
        if (token !== this.token || this.pages.get(index) !== record) return;
        for (const widget of widgets) {
          const field = this.fields.get(widget.field); if (!field) throw new Error('Unknown form field');
          const control = widgetControl(this.doc, field, widget, (change) => this.change(change));
          if (control) {
            control.applyStyle = (style) => { if (!style) return; control.textStyle = style; if (this.doc.activeElement !== control.node) applyTextStyle(this.doc, control.node, widget, style); };
            control.node.addEventListener('blur', () => { if (control.textStyle) applyTextStyle(this.doc, control.node, widget, control.textStyle); });
            control.applyStyle(styles.get(widget.index)); record.controls.push(control); layer.append(control.node); control.sync(this.changes.get(field.index));
          }
        }
      } catch (error) { if (token === this.token) this.message.textContent = `Form controls unavailable: ${error}`; }
    }
    for (const control of record.controls) positionWidget(control.node, control.widget.rect, info, zoom);
    await this.buttons?.page(index, record, info, zoom);
    await this.preview?.page(index, record, info, zoom);
    record.layer.inert = this.busy; record.layer.hidden = !this.active;
  }
  change(change) {
    if (this.busy) return;
    this.changes.set(change.index, change); this.sync(); if (this.fields.get(change.index)?.kind !== 2) this.preview?.schedule();
    this.message.textContent = 'Unsaved changes'; this.controls();
  }
  sync() { for (const record of this.pages.values()) for (const control of record.controls) control.sync(this.changes.get(control.field)); }
  async save() {
    if (this.busy) return;
    if (!this.changes.size) { this.download(); return; }
    const state = this.state(), token = this.token, editing = this.editing;
    if (!editing) return;
    this.preview?.clear();
    this.busy = true; this.controls(); this.message.textContent = 'Saving PDF…';
    try {
      const bytes = await editing.client.saveForm(editing.handle, [...this.changes.values()].sort((a, b) => a.index - b.index));
      if (token !== this.token) return;
      const adopted = await this.adopt(bytes, state.epoch);
      if (!adopted || token !== this.token) return;
      this.changes.clear(); this.download();
      const loading = this.open(), refreshed = this.token;
      if (await loading && refreshed === this.token) this.message.textContent = 'PDF saved. You can keep editing.';
    } catch (error) { if (token === this.token) this.message.textContent = `Could not save; your changes are still here. ${error}`; }
    finally { if (token === this.token) { this.busy = false; this.controls(); } }
  }
}
