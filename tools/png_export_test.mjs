/** Hosted PNG wire parity and explicit viewer-export lifecycle controls. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { decodeResponse, encodeRequest, FrameDecoder, VERSION_1_9 } from '../web/protocol.mjs';
import { PngBackground, RequestKind } from '../web/protocol_constants.mjs';
import { PngExport, exportPagePng } from '../web/png_export.mjs';
import { downloadDocumentBytes } from '../web/document_services.mjs';

function parse(bytes) { const frames = [], decoder = new FrameDecoder(frame => frames.push(frame)); decoder.feed(bytes); decoder.finish(); return frames; }
const signature = Uint8Array.of(137, 80, 78, 71, 13, 10, 26, 10);

export async function testPngExport(client, root, native, wasiRun) {
  const request = { document: 1, page: 0, scale: 1, background: PngBackground.Transparent };
  assert.deepEqual(encodeRequest(26, 3, request), Uint8Array.of(22, 0, 0, 0, 26, 3, 0, 0, 0, 1, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 240, 63, 1), 'independent literal PNG request');
  for (const change of [{ background: 2 }, { background: true }, { page: -1 }, { scale: 0 }, { scale: Infinity }]) {
    assert.throws(() => encodeRequest(26, 3, { ...request, ...change }));
  }
  const response = Uint8Array.of(154, 3, 0, 0, 0, 8, 0, 0, 0, ...signature);
  assert.deepEqual(decodeResponse(response).value, signature);
  for (let n = 0; n < response.length; n++) assert.throws(() => decodeResponse(response.subarray(0, n)));
  assert.throws(() => decodeResponse(Uint8Array.of(...response, 0)));
  for (const name of ['blank', 'rectangle', 'alpha-unsupported']) {
    const bytes = new Uint8Array(await readFile(join(root, 'corpus/generated/classic', `${name}.pdf`)));
    const input = Buffer.concat([encodeRequest(1, 1, { version: VERSION_1_9 }),
      encodeRequest(2, 2, { bytes, recovery: 0, password: new Uint8Array() }),
      encodeRequest(RequestKind.PagePng, 3, { ...request, background: PngBackground.White }), encodeRequest(RequestKind.PagePng, 4, request)]);
    const result = spawnSync(native, [], { input, timeout: 30000 }); assert.ifError(result.error); assert.equal(result.status, 0, result.stderr.toString());
    const wasi = await wasiRun(input, `png-${name}`); assert.equal(wasi.code, 0, wasi.stderr.toString());
    assert.deepEqual(wasi.bytes, result.stdout, 'PNG native/WASI response identity');
    const frames = parse(wasi.bytes); assert.equal(frames.length, 4);
    const opened = await client.open(bytes, 0);
    for (const background of [PngBackground.White, PngBackground.Transparent]) {
      const png = await client.pagePng(opened.document, 0, 1, background);
      assert.deepEqual(png, frames[2 + background].value, 'client bytes equal independently pixel-checked native PNG');
      assert.deepEqual(png.subarray(0, 8), signature);
      assert.deepEqual([...png.subarray(16, 29)], [0, 0, 0, 64, 0, 0, 0, 64, 8, 6, 0, 0, 0]);
    }
    await assert.rejects(client.pagePng(opened.document, 99), error => error.category === 7);
    assert.equal((await client.pageInfo(opened.document, 0)).width, 64);
    await client.close(opened.document);
  }
  console.log('PNG host: canonical wire, malformed payloads and native/WASI/client byte parity passed; Python checks independent pixels.');
}

function dom() {
  class Element {
    constructor() { this.listeners = new Map(); this.value = ''; this.checked = false; this.disabled = false; this.textContent = ''; this.open = false; this.children = []; }
    addEventListener(type, callback) { this.listeners.set(type, callback); }
    fire(type) { this.listeners.get(type)?.({ preventDefault() {} }); }
    showModal() { this.open = true; }
    close() { this.open = false; this.fire('close'); }
    focus() { this.focused = true; }
    append(node) { this.children.push(node); }
    click() { this.clicked = true; }
    remove() { this.removed = true; }
  }
  const elements = new Map();
  return { createElement: () => new Element(), body: new Element(), getElementById: id => {
    if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id);
  } };
}

export async function testPngExportView() {
  const document = dom(), calls = [], downloads = [];
  let finish, hold = false, fail = false;
  const client = { pagePng: async (...args) => { calls.push(args); if (hold) await new Promise(resolve => { finish = resolve; });
    if (fail) throw new Error('PNG budget exceeded'); return signature; } };
  const state = { client, handle: null, epoch: 1, count: 2, page: 1 };
  const editor = { busy: false, changes: new Map(), editing: null };
  const view = new PngExport({ state: () => state, editor, document, download: (bytes, name, doc, type) => downloads.push({ bytes, name, doc, type }) });
  view.controls(); assert.equal(view.openButton.disabled, true);
  state.handle = 1; view.controls(); assert.equal(view.openButton.disabled, false); view.show();
  assert.equal(view.page.value, '2'); assert.equal(view.page.max, '2'); view.dpi.value = '144';
  await view.export(); assert.deepEqual(calls, [[1, 1, 2, 0]]);
  assert.equal(downloads[0].name, 'page-0002.png'); assert.equal(downloads[0].type, 'image/png');
  view.transparent.checked = true; await view.export(); assert.deepEqual(calls.at(-1), [1, 1, 2, 1]);
  for (const [page, dpi] of [['0', '144'], ['3', '144'], ['1.5', '144'], ['1', '0'], ['1', '2401'], ['1', 'NaN']]) {
    view.page.value = page; view.dpi.value = dpi; const count = calls.length; await view.export(); assert.equal(calls.length, count);
    assert.match(view.message.textContent, /Choose a page/);
  }
  view.page.value = '1'; view.dpi.value = '72'; fail = true; await view.export();
  assert.match(view.message.textContent, /budget exceeded/); assert.equal(view.submit.disabled, false); assert.equal(downloads.length, 2); fail = false;
  for (const stale of [() => state.epoch++, () => state.handle++, () => view.dialog.close(), () => { state.client = { ...client }; }]) {
    view.show(); hold = true; const exporting = view.export(); const count = calls.length;
    await view.export(); assert.equal(calls.length, count, 'duplicate export is bounded');
    stale(); finish(); await exporting; assert.equal(downloads.length, 2, 'stale or closed export cannot download');
    assert.equal(view.submit.disabled, false); state.client = client;
  }
  hold = false; view.reset(); assert.equal(view.dialog.open, false);
  await snapshotChecks(); downloadCheck(document);
  console.log('PNG viewer: explicit options, bounded requests, failures, stale downloads, immutable draft snapshots and handle/URL cleanup passed.');
}

async function snapshotChecks() {
  let events = [], fail = false, active = true, finishSave, holdSave = false, finishOpen, holdOpen = false;
  const client = {
    saveForm: async (handle, changes) => { events.push(['save', handle, changes]); if (holdSave) await new Promise(resolve => { finishSave = resolve; }); return Uint8Array.of(7); },
    open: async (bytes, recovery) => { events.push(['open', [...bytes], recovery]); if (holdOpen) await new Promise(resolve => { finishOpen = resolve; }); return { document: 20 }; },
    pagePng: async (...args) => { events.push(['png', ...args]); if (fail) throw new Error('paint failed'); return signature; },
    close: async handle => { events.push(['close', handle]); },
  };
  const changes = new Map([[3, { index: 3, kind: 0, value: 'Draft' }]]);
  const editor = { busy: false, changes, editing: { client, handle: 10 } }, snapshot = { client, handle: 1 };
  const options = { page: 0, scale: 2, background: 1 }, current = () => active;
  assert.equal(await exportPagePng(snapshot, options, editor, current), signature);
  assert.deepEqual(events, [['save', 10, [{ index: 3, kind: 0, value: 'Draft' }]], ['open', [7], 1], ['png', 20, 0, 2, 1], ['close', 20]]);
  assert.equal(changes.get(3).value, 'Draft', 'export does not clear or adopt drafts');
  fail = true; events = []; await assert.rejects(exportPagePng(snapshot, options, editor, current), /paint failed/);
  assert.deepEqual(events.at(-1), ['close', 20], 'failed rendering releases the temporary document'); fail = false;
  events = []; holdSave = true; const pending = exportPagePng(snapshot, options, editor, current);
  changes.get(3).value = 'Later typing'; active = false; finishSave(); assert.equal(await pending, null);
  assert.equal(events.length, 1, 'stale save cannot open another document'); assert.equal(events[0][2][0].value, 'Draft', 'click-time values are copied');
  holdSave = false; holdOpen = true; active = true; events = [];
  const opening = exportPagePng(snapshot, options, editor, current); await Promise.resolve();
  active = false; finishOpen(); assert.equal(await opening, null); assert.deepEqual(events.at(-1), ['close', 20], 'stale open is released');
  holdOpen = false; active = true; editor.busy = true;
  await assert.rejects(exportPagePng(snapshot, options, editor, current), /current PDF save/);
  editor.busy = false; editor.editing = null; await assert.rejects(exportPagePng(snapshot, options, editor, current), /not ready/);
}

function downloadCheck(document) {
  const original = { create: URL.createObjectURL, revoke: URL.revokeObjectURL, timeout: globalThis.setTimeout };
  let blob, release, revoked;
  URL.createObjectURL = value => { blob = value; return 'blob:png'; };
  URL.revokeObjectURL = value => { revoked = value; };
  globalThis.setTimeout = callback => { release = callback; return 1; };
  try {
    downloadDocumentBytes(signature, 'page-0001.png', document, 'image/png');
    assert.equal(blob.type, 'image/png'); const link = document.body.children.at(-1);
    assert.equal(link.download, 'page-0001.png'); assert.equal(link.clicked, true); assert.equal(link.removed, true);
    release(); assert.equal(revoked, 'blob:png');
  } finally { URL.createObjectURL = original.create; URL.revokeObjectURL = original.revoke; globalThis.setTimeout = original.timeout; }
}
