import assert from 'node:assert/strict';
import { FormPreview } from '../web/form_preview.mjs';

export async function testFormPreview() {
  const closed = [], saved = [], rendered = [], flags = new Map();
  let finish, fail = false;
  const client = {
    saveForm: async (handle, changes) => { saved.push({ handle, changes }); if (fail) throw new Error('Bad draft'); return new Promise((resolve) => { finish = resolve; }); },
    open: async (_, recovery) => { assert.equal(recovery, 1); return { document: 42 }; },
    close: async (handle) => { closed.push(handle); },
    renderTile: async (handle, index, scale, tile) => { rendered.push({ handle, index, scale, tile }); return { width: tile.width, height: tile.height, rgba: new Uint8Array(tile.width * tile.height * 4) }; },
  };
  const canvases = [], pixels = [];
  const doc = { createElement: () => {
    const canvas = { style: {}, setAttribute() {}, remove() { this.removed = true; }, getContext: () => ({ putImageData: (image) => pixels.push(image) }) };
    canvases.push(canvas); return canvas;
  } };
  const record = { layer: { append() {} }, controls: [{ field: 2, widget: { rect: [10, 20, 30, 40] }, node: { classList: { toggle: (name, state) => flags.set(name, state) } } }] };
  const editor = { changes: new Map([[2, { index: 2, kind: 2 }]]), editing: { client, handle: 9 }, pages: new Map([[0, record]]), doc, redraw() {}, message: {} };
  const preview = new FormPreview(editor);
  const stale = preview.update(); preview.clear(); finish(Uint8Array.of(1)); await stale;
  assert.equal(preview.snapshot, null, 'discard during save cannot resurrect a draft');
  const updated = preview.update(); finish(Uint8Array.of(1)); await updated;
  const priorImageData = globalThis.ImageData;
  globalThis.ImageData = class { constructor(rgba, width, height) { Object.assign(this, { rgba, width, height }); } };
  try {
    const info = { crop: [0, 0, 100, 100], rotation: 90, userUnit: 1, width: 100, height: 100 };
    await preview.page(0, record, info, 2);
    assert.deepEqual(rendered[0], { handle: 42, index: 0, scale: 2, tile: { x: 40, y: 20, width: 40, height: 40 } });
    assert.equal(flags.get('form-previewed'), true, 'native control yields to actual PDF artwork');
    await preview.page(0, record, info, 2); assert.equal(rendered.length, 1, 'unchanged draft does not rerender');
    await preview.page(0, record, info, 1); assert.equal(rendered.length, 2, 'zoom regenerates aligned pixels');
    assert.equal(canvases[0].removed, true);
    await preview.page(0, record, info, 0.5);
    assert.deepEqual(rendered.at(-1), { handle: 42, index: 0, scale: 2, tile: { x: 40, y: 20, width: 40, height: 40 } });
    assert.equal(canvases.at(-1).style.left, '10px');
    assert.equal(canvases.at(-1).style.top, '5px');
    assert.equal(canvases.at(-1).style.width, '10px', 'supersampled draft stays aligned to the reduced rotated widget');
  } finally { globalThis.ImageData = priorImageData; }
  preview.clear(); assert.deepEqual(closed, [42]); assert.equal(flags.get('form-previewed'), false);
  assert.equal(saved[1].handle, 9, 'every preview starts from the saved source, not a previous draft');
  fail = true; await preview.update(); assert.match(editor.message.textContent, /Draft retained/);
  assert.equal(editor.changes.size, 1, 'failed preview preserves edits');
  console.log('Draft previews: actual artwork, rotation/zoom, stale discard, snapshot cleanup and failure retention passed.');
}
