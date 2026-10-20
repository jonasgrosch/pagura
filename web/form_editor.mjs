/** Browser-owned draft controls; the host never retains a mutable document. */
export class FormEditor {
  constructor({ state, adopt, download, document: doc = document }) {
    this.state = state; this.adopt = adopt; this.download = download; this.doc = doc;
    this.panel = doc.getElementById('form-panel'); this.list = doc.getElementById('form-fields');
    this.message = doc.getElementById('form-status'); this.applyButton = doc.getElementById('form-apply');
    this.downloadButton = doc.getElementById('form-download'); this.openButton = doc.getElementById('edit-open');
    this.changes = new Map(); this.token = 0; this.active = false; this.busy = false;
    this.openButton.addEventListener('click', () => { this.active = !this.active; this.panel.hidden = !this.active; if (this.active) void this.open(); });
    this.applyButton.addEventListener('click', () => { void this.apply(); });
    doc.getElementById('form-reset').addEventListener('click', () => { void this.open(); });
    this.downloadButton.addEventListener('click', () => { if (!this.changes.size && !this.busy) this.download(); });
  }
  controls() {
    const { handle } = this.state(); this.openButton.disabled = handle === null;
    this.applyButton.disabled = this.busy || !this.changes.size;
    this.downloadButton.disabled = this.busy || this.changes.size > 0 || handle === null;
    this.doc.getElementById('form-reset').disabled = this.busy;
  }
  reset() { ++this.token; this.changes.clear(); this.list.replaceChildren(); this.list.inert = false; this.busy = false; this.controls(); }
  recovered() {
    ++this.token; this.busy = false; this.list.inert = false; this.controls();
    if (this.changes.size) this.message.textContent = 'The PDF host restarted. Your draft is still here; apply it when ready.';
    else void this.open();
  }
  async open() {
    this.reset(); if (!this.active) return;
    this.panel.hidden = false;
    const token = this.token, state = this.state();
    if (state.handle === null) { this.message.textContent = 'Open a PDF to edit its form.'; return; }
    if (state.recovery !== 0) { this.message.textContent = 'Enable Strict validation and reopen the PDF to edit its form.'; return; }
    this.message.textContent = 'Loading form fields…';
    try {
      const fields = await state.client.formFields(state.handle);
      if (token !== this.token) return;
      for (const field of fields) this.list.append(this.control(field));
      this.message.textContent = fields.length ? 'Edit the fields, then apply changes to update the preview.' : 'This PDF has no fillable form fields.';
    } catch (error) { if (token === this.token) this.message.textContent = String(error); }
    this.controls();
  }
  control(field) {
    const doc = this.doc, wrapper = doc.createElement('label'), title = doc.createElement('span');
    title.textContent = field.name || `Field ${field.index + 1}`; wrapper.append(title);
    const disabled = Boolean(field.flags & 1) || field.kind === 3 || (field.kind === 2 && !field.options.length);
    let input, change;
    if (field.kind === 0 || (field.kind === 1 && field.flags & 8)) {
      input = doc.createElement(field.flags & 4 ? 'textarea' : 'input'); input.value = field.value;
      if (field.kind === 1 && !input.value) input.value = field.options.find((option) => option.selected)?.label ?? '';
      input.maxLength = 8 * 1024 * 1024;
      change = () => ({ index: field.index, kind: field.kind === 0 ? 0 : 4, value: input.value });
    } else if (field.kind === 1 || field.kind === 2) {
      input = doc.createElement('select'); input.multiple = field.kind === 1 && Boolean(field.flags & 2);
      if (!input.multiple) { const empty = doc.createElement('option'); empty.value = ''; empty.textContent = field.kind === 2 ? 'Off' : 'No selection'; input.append(empty); }
      for (const [ordinal, option] of field.options.entries()) {
        const item = doc.createElement('option'); item.value = String(ordinal); item.textContent = option.label; item.selected = option.selected; input.append(item);
      }
      change = () => {
        const selected = Array.from(input.selectedOptions).filter((item) => item.value !== '').map((item) => field.options[Number(item.value)]);
        if (field.kind === 1) return { index: field.index, kind: 1, indices: selected.map((option) => option.key) };
        return selected.length ? { index: field.index, kind: 3, widget: selected[0].key, state: selected[0].state } : { index: field.index, kind: 2 };
      };
    } else { input = doc.createElement('input'); input.value = 'Not editable'; }
    input.id = `form-field-${field.index}`; input.disabled = disabled;
    input.addEventListener('input', () => { if (disabled || this.busy) return; this.changes.set(field.index, change()); this.message.textContent = 'Unapplied changes. Apply to preview and download.'; this.controls(); });
    wrapper.append(input); return wrapper;
  }
  async apply() {
    if (this.busy || !this.changes.size) return;
    const state = this.state(), token = this.token;
    this.busy = true; this.controls(); this.list.inert = true; this.message.textContent = 'Saving changes…';
    try {
      const bytes = await state.client.saveForm(state.handle, [...this.changes.values()].sort((a, b) => a.index - b.index));
      if (token !== this.token) return;
      const adopted = await this.adopt(bytes, state.epoch);
      if (!adopted || token !== this.token) return;
      this.busy = false; this.list.inert = false;
      const loading = this.open(), refreshed = this.token;
      await loading;
      if (refreshed === this.token) this.message.textContent = 'Changes applied. The updated PDF is ready to download.';
    } catch (error) { if (token === this.token) this.message.textContent = String(error); }
    finally { if (token === this.token) { this.busy = false; this.list.inert = false; this.controls(); } }
  }
}
