/** Behavioral checks for virtualized inline editing and strict-save snapshots. */
import assert from 'node:assert/strict';
import { FormEditor } from '../web/form_editor.mjs';
import { positionWidget, widgetControl } from '../web/form_layer.mjs';
export async function testFormEditor() {
  class Element {
    constructor(tag = 'div') { this.tagName = tag.toUpperCase(); }
    children = []; listeners = new Map(); textContent = ''; value = ''; style = {}; attributes = new Map();
    classList = { toggle() {} };
    append(...items) { this.children.push(...items); for (const item of items) item.parent = this; }
    remove() { if (this.parent) this.parent.children = this.parent.children.filter((child) => child !== this); }
    setAttribute(key, value) { this.attributes.set(key, value); }
    addEventListener(type, callback) { this.listeners.set(type, callback); }
    fire(type) { this.listeners.get(type)?.(); }
  }
  const elements = new Map(), document = { createElement: (tag) => new Element(tag), getElementById: (id) => {
    if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id);
  } };
  const field = { index: 0, kind: 0, flags: 0, name: 'Name', value: '', options: [] };
  const widget = { index: 0, field: 0, flags: 0, rect: [40, 300, 380, 328] };
  const info = { crop: [0, 0, 600, 440], rotation: 0, userUnit: 1 };
  let epoch = 0, handle = 1, adopted = 0, downloaded = 0, fail = true, finishSave, wait = false, strictOpens = 0;
  const client = {
    open: async (_, recovery) => { assert.equal(recovery, 0); strictOpens++; return { document: 9 }; }, close: async () => {},
    formFields: async () => [field], formWidgets: async (handle) => { assert.equal(handle, 9); return [widget]; },
    saveForm: async (handle) => { assert.equal(handle, 9); if (fail) throw new Error('Unsupported appearance'); if (wait) await new Promise((resolve) => { finishSave = resolve; }); return Uint8Array.of(1); },
  };
  const editor = new FormEditor({ document, preview: false, state: () => ({ client, handle, epoch, recovery: 1, bytes: Uint8Array.of(1) }),
    adopt: async () => { adopted++; epoch++; return true; }, download: () => { downloaded++; } });
  const parent = new Element(); await editor.open(); await editor.page(0, info, parent, 1);
  const input = editor.pages.get(0).controls[0].node; input.value = 'Draft'; input.fire('input');
  assert.equal(input.style.transform, 'matrix(1,0,0,1,40,112)');
  assert.equal(editor.saveButton.disabled, false);
  editor.openButton.fire('click'); editor.openButton.fire('click');
  assert.equal(editor.changes.get(0).value, 'Draft');
  editor.prune([]); await editor.page(0, info, new Element(), 2);
  assert.equal(editor.pages.get(0).controls[0].node.value, 'Draft', 'scroll/zoom virtualization keeps draft');
  assert.equal(editor.pages.get(0).controls[0].node.style.transform, 'matrix(2,0,0,2,80,224)');
  await editor.save(); assert.match(editor.message.textContent, /Unsupported appearance/); assert.equal(adopted, 0); assert.equal(downloaded, 0);
  assert.equal(editor.changes.get(0).value, 'Draft'); assert.equal(editor.busy, false);
  fail = false; wait = true; const applying = editor.save(); await Promise.resolve();
  epoch++; handle = 2; editor.reset(); finishSave(); await applying;
  assert.equal(adopted, 0, 'stale save must not replace a newly opened file');
  await editor.open(); editor.change({ index: 0, kind: 0, value: 'New' }); wait = false; await editor.save();
  assert.equal(adopted, 1); assert.equal(downloaded, 1); assert.equal(editor.changes.size, 0); assert.ok(strictOpens >= 2);
  assert.match(editor.message.textContent, /PDF saved/);
  // Independent expected transforms: cropped, scaled and rotated page coordinates.
  const expected = ['matrix(2,0,0,2,20,140)', 'matrix(0,2,-2,0,260,20)', 'matrix(-2,0,0,-2,380,260)', 'matrix(0,-2,2,0,140,380)'];
  for (const [i, rotation] of [0, 90, 180, 270].entries()) {
    const node = new Element(); positionWidget(node, [20, 100, 60, 150], { crop: [10, 20, 210, 220], userUnit: 2, rotation }, 1);
    assert.equal(node.style.transform, expected[i]); assert.equal(node.style.width, '40px'); assert.equal(node.style.height, '50px');
  }
  const button = { ...field, index: 1, kind: 2, options: [{ key: 3, label: 'Email', state: Uint8Array.of(69), selected: false }, { key: 4, label: 'Phone', state: Uint8Array.of(80), selected: true }] };
  const radio = widgetControl(document, button, { ...widget, index: 3, flags: 1 }, () => {});
  assert.equal(radio.node.type, 'radio'); radio.sync({ kind: 3, widget: 3, state: Uint8Array.of(69) }); assert.equal(radio.node.checked, true);
  radio.sync({ kind: 3, widget: 4, state: Uint8Array.of(80) }); assert.equal(radio.node.checked, false);
  assert.equal(widgetControl(document, { ...field, flags: 1 }, widget, () => {}), null);
  console.log('Inline editing: transforms, virtualized drafts, strict snapshots, atomic save/download and radio synchronization passed.');
}
