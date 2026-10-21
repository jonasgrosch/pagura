import assert from 'node:assert/strict';
import { ButtonArtwork } from '../web/form_buttons.mjs';
import { widgetControl } from '../web/form_layer.mjs';
import { applyTextStyle } from '../web/form_typography.mjs';
export async function testButtonArtwork() {
  let saves = 0, renders = 0, opens = 0, closes = 0;
  const doc = { createElement(tag) { return { tagName: tag.toUpperCase(), style: {}, classList: { toggle() {} }, listeners: {}, setAttribute() {}, remove() {},
    addEventListener(name, fn) { this.listeners[name] = fn; }, getContext: () => ({ putImageData() {}, measureText: () => ({ fontBoundingBoxAscent: 9, fontBoundingBoxDescent: 2 }) }) }; } };
  const field = { index: 0, kind: 2, flags: 0, name: 'Check', options: [{ key: 0, selected: false, state: Uint8Array.of(89) }] };
  const widget = { index: 0, field: 0, flags: 0, rect: [10, 20, 12, 22] };
  const editor = { doc, fields: new Map([[0,field]]), changes: new Map(), pages: new Map(), message: {}, sync() { control.sync(this.changes.get(0)); } };
  const control = widgetControl(doc, field, widget, change => { editor.changes.set(0, change); editor.sync(); });
  const record = { controls: [control], layer: { append() {} } }; editor.pages.set(0, record);
  const client = { saveForm: async () => { saves++; return Uint8Array.of(1); }, open: async () => ({ document: ++opens }), close: async () => { closes++; }, renderTile: async (_, __, ___, tile) => { renders++; return { width: tile.width, height: tile.height, rgba: new Uint8Array(tile.width * tile.height * 4) }; } };
  editor.editing = { client, handle: 9 };
  const oldImageData = globalThis.ImageData;
  globalThis.ImageData = class {};
  try {
    await new ButtonArtwork(editor).page(0, record, { crop: [0,0,100,100], width: 100, height: 100, userUnit: 1, rotation: 0 }, 1);
    assert.equal(control.node.disabled, false); assert.equal(saves, 2); assert.equal(renders, 2); assert.equal(closes, 2);
    control.node.checked = true; control.node.listeners.input();
    assert.equal(control.artwork.get(true).hidden, false, 'on artwork is visible in the same input event');
    assert.equal(control.artwork.get(false).hidden, true);
    control.node.checked = false; control.node.listeners.input();
    assert.equal(control.artwork.get(false).hidden, false, 'off artwork is immediate too');
    assert.equal(saves, 2, 'clicking never saves, opens or invokes WASM'); assert.equal(renders, 2); assert.equal(opens, 2);
  } finally { globalThis.ImageData = oldImageData; }
  const input = doc.createElement('input');
  applyTextStyle(doc, input, { rect: [0,0,100,15] }, { font: '/AAAAAA+Helvetica', size: 10, left: 3, baseline: 10, align: 0 });
  assert.equal(input.style.fontSize, '10px'); assert.equal(input.style.paddingLeft, '2px'); assert.equal(input.style.paddingTop, '0px'); assert.equal(input.style.paddingBottom, '2px');
  console.log('Button artwork switches synchronously without host work; text uses stored font size and baseline.');
}
