import { selectionPath, paintSelection, registerSelectionGeometry } from '../web/selection_layer.mjs';
import assert from 'node:assert/strict';
import { rotatedSize, rotatePoint, unrotatePoint, rasterBounds } from '../web/view_rotation.mjs';
import { Thumbnails } from '../web/thumbnails.mjs';

// Non-square pages and partial viewports catch the missing/incorrect rotated tiles.
for (const rotation of [0, 90, 180, 270]) {
  for (const [x, y] of [[0, 0], [600, 800], [17, 219]]) {
    assert.deepEqual(unrotatePoint(...rotatePoint(x, y, 600, 800, rotation), 600, 800, rotation), [x, y]);
  }
  const size = rotatedSize(600, 800, rotation);
  assert.deepEqual(rasterBounds({ left: 20, top: 30 }, { left: 0, top: 0, right: size.width + 40, bottom: size.height + 60 }, 600, 800, rotation), { left: 0, top: 0, right: 600, bottom: 800 });
}
assert.deepEqual(rasterBounds({ left: 0, top: 0 }, { left: 20, top: 30, right: 80, bottom: 90 }, 600, 800, 270), { left: 510, top: 20, right: 570, bottom: 80 });
assert.equal(rasterBounds({ left: 0, top: 0 }, { left: 900, top: 900, right: 1000, bottom: 1000 }, 600, 800, 90), null);

const thumbnailDraws = [];
class Element {
  constructor() { this.children = []; this.style = {}; this.attributes = {}; this.scrollTop = 0; this.clientHeight = 600; }
  addEventListener() {}
  append(...nodes) { for (const node of nodes) { node.remove(); this.children.push(node); node.parent = this; } }
  replaceChildren(...nodes) { this.children.forEach(node => { node.parent = null; }); this.children = []; this.append(...nodes); }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter(node => node !== this); }
  setAttribute(key, value) { this.attributes[key] = value; }
  removeAttribute(key) { delete this.attributes[key]; }
  get firstChild() { return this.children[0]; }
  getContext() { return { putImageData() {}, drawImage(...args) { thumbnailDraws.push(args); } }; }
}
const original = { document: globalThis.document, ImageData: globalThis.ImageData };
globalThis.document = { createElement: () => new Element() }; globalThis.ImageData = class {};
let renders = 0, metadata = 0, pending;
const state = { epoch: 1, count: 2000, page: 0, visible: true, handle: 1, idle: false, label: index => String(index + 1), rotation: () => 0,
  client: { pageInfo: async () => { metadata++; return { crop: [0,0,600,800], userUnit: 1, rotation: 0 }; }, renderTile: async (_, __, ___, tile) => {
    renders++; if (pending) await pending.promise;
    return { ...tile, rgba: new Uint8Array(tile.width * tile.height * 4) };
  } } };
const list = new Element(), panel = new Element(), thumbnails = new Thumbnails({ state: () => state, navigate() {}, list, panel });
// Explicitly drive scheduling without wall-clock timers in this unit test.
thumbnails.schedule = () => {};
try {
  thumbnails.refresh();
  assert.ok(list.children.length <= 6, 'thousands of pages must not create thousands of DOM rows');
  await thumbnails.renderNext(); assert.equal(metadata, 0, 'foreground rendering blocks thumbnail work');
  state.idle = true; await thumbnails.renderNext(); assert.equal(renders, 1);
  await thumbnails.renderNext(); assert.equal(renders, 2); assert.equal(thumbnails.cache.size, 2);
  const firstThumbnail = thumbnails.cache.get(0);
  assert.equal(firstThumbnail.canvas.height, 332, 'thumbnails render at two physical pixels per CSS pixel');
  const patch = new Element(); Object.assign(patch.style, { left:'10px', top:'20px', width:'30px', height:'40px' });
  const record = { info:{width:600,height:800}, zoom:1, layer:new Element(), buttonCanvases:[patch], previewCanvases:[] };
  state.forms = index => index === 0 ? record : undefined; state.dirtyForms = true;
  thumbnails.revision++; thumbnails.paint(0, thumbnails.rows.get(0));
  assert.equal(renders, 2, 'draft compositing reuses field pixels without a host render or save');
  const composed = thumbnailDraws.at(-1);
  assert.equal(composed[0], patch); assert.ok(Math.abs(composed[1] - 4.15) < 1e-6); assert.ok(Math.abs(composed[2] - 8.3) < 1e-6);
  state.forms = () => undefined; state.dirtyForms = false; thumbnails.revision++; thumbnails.paint(0, thumbnails.rows.get(0));
  assert.equal(firstThumbnail.composite, null, 'discard/reset removes stale form artwork');
  state.visible = false; await thumbnails.renderNext(); assert.equal(renders, 2, 'hidden sidebar does no work');
  state.visible = true; let release;
  pending = { promise: new Promise(resolve => { release = resolve; }) };
  const job = thumbnails.renderNext(); await Promise.resolve(); await Promise.resolve();
  state.epoch++; thumbnails.refresh(); release(); await job;
  assert.equal(thumbnails.cache.size, 0, 'results from a previous document cannot enter the new cache');
  pending = null;
  for (let i = 0; i < 40; i++) { panel.scrollTop = i * 202 * 7; thumbnails.refresh(); await thumbnails.renderNext(); }
  assert.equal(thumbnails.cache.size, 32, 'thumbnail pixel memory remains bounded');
  assert.ok(list.children.length <= 6, 'scrolling evicts offscreen DOM');
  panel.clientHeight = 20000; thumbnails.refresh(); assert.ok(list.children.length <= 32, 'extreme viewport sizes retain the thumbnail memory bound');
  state.epoch++; thumbnails.refresh();
  state.client.renderTile = async () => { throw new Error('unsupported page'); };
  await thumbnails.renderNext(); assert.equal(thumbnails.cache.values().next().value.error, true);
  thumbnails.rows.clear(); list.replaceChildren(); thumbnails.refresh(); // cached failures survive virtualization safely
  thumbnails.destroy();
  assert.equal(thumbnails.cache.size, 0);
} finally { thumbnails.destroy(); Object.assign(globalThis, original); }
console.log('Viewer rotation, thumbnail priority, virtualization, epoch and cache bounds passed');

// Mixed/mirrored glyph orientations share one winding and one translucent fill.
const quad = [[0,0],[10,0],[10,12],[0,12]];
assert.equal(selectionPath([quad]), selectionPath([[...quad].reverse()]));
const selectionDoc = { createElementNS: () => new Element() };
const parent = new Element(), layer = new Element(); layer.parentElement = parent; parent.append(layer);
layer.closest = () => layer;
const spanA = { textContent: 'A', firstChild: { nodeType: 3 }, childNodes: [1] };
const spanB = { textContent: 'B', firstChild: { nodeType: 3 }, childNodes: [1] };
spanA.firstChild.parentElement = spanB.firstChild.parentElement = layer;
registerSelectionGeometry(layer, [{ span: spanA, quads: [quad] }, { span: spanB, quads: [[[8,0],[18,0],[18,12],[8,12]]] }]);
const range = { startContainer: spanA.firstChild, startOffset: 0, endContainer: spanB.firstChild, endOffset: 1, intersectsNode: () => true };
const selection = { isCollapsed: false, rangeCount: 1, getRangeAt: () => range };
paintSelection(selection, [layer], selectionDoc);
assert.equal(parent.children.length, 2); assert.equal(parent.children[1].children.length, 1, 'overlapping glyphs use one fill, not stacked highlight elements');
assert.equal((parent.children[1].firstChild.attributes.d.match(/M/g) ?? []).length, 2);
range.endContainer = spanB; spanB.closest = () => layer; range.endOffset = 0;
paintSelection(selection, [layer], selectionDoc);
assert.equal((parent.children[1].firstChild.attributes.d.match(/M/g) ?? []).length, 1, 'end at a span boundary excludes that character');
selection.isCollapsed = true; paintSelection(selection, [layer], selectionDoc); assert.equal(parent.children.length, 1);
console.log('Selection winding, boundary and overlay lifecycle passed');
