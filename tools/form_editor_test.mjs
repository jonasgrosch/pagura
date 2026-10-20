/** Behavioral editor checks: drafts, failed saves and stale asynchronous work. */
import assert from 'node:assert/strict';
import { FormEditor } from '../web/form_editor.mjs';
export async function testFormEditor() {
  class Element {
    children = []; listeners = new Map(); textContent = ''; value = '';
    append(...items) { this.children.push(...items); }
    replaceChildren(...items) { this.children = items; }
    addEventListener(type, callback) { this.listeners.set(type, callback); }
    fire(type) { this.listeners.get(type)?.(); }
  }
  const elements = new Map();
  const document = { createElement: () => new Element(), getElementById: (id) => {
    if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id);
  } };
  const field = { index: 0, kind: 0, flags: 0, name: 'Name', value: '', options: [] };
  let epoch = 0, handle = 1, adopted = 0, fail = true, finishSave, wait = false;
  const client = {
    formFields: async () => [field],
    saveForm: async () => { if (fail) throw new Error('Unsupported appearance'); if (wait) await new Promise((resolve) => { finishSave = resolve; }); return Uint8Array.of(1); },
  };
  const editor = new FormEditor({ document, state: () => ({ client, handle, epoch, recovery: 0 }),
    adopt: async () => { adopted++; epoch++; return true; }, download: () => {} });
  editor.active = true; await editor.open();
  const input = editor.list.children[0].children[1]; input.value = 'Draft'; input.fire('input');
  assert.equal(editor.applyButton.disabled, false); assert.equal(editor.downloadButton.disabled, true);
  editor.openButton.fire('click'); editor.openButton.fire('click');
  assert.equal(editor.changes.get(0).value, 'Draft', 'panel toggle retains draft');
  await editor.apply();
  assert.match(editor.message.textContent, /Unsupported appearance/); assert.equal(adopted, 0);
  assert.equal(editor.changes.get(0).value, 'Draft'); assert.equal(editor.busy, false);
  fail = false; wait = true;
  const applying = editor.apply(); await Promise.resolve();
  epoch++; handle = 2; editor.reset(); finishSave(); await applying;
  assert.equal(adopted, 0, 'old save must not replace a newly opened file');
  await editor.open(); const next = editor.list.children[0].children[1]; next.value = 'New'; next.fire('input');
  wait = false; await editor.apply();
  assert.equal(adopted, 1); assert.equal(editor.changes.size, 0); assert.equal(editor.downloadButton.disabled, false);
  assert.match(editor.message.textContent, /ready to download/);
  console.log('Form editor: draft preservation, failure recovery, stale saves and download gating passed.');
}
