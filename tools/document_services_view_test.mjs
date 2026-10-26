/** Behavioral checks for explicit downloads, inert labels and document/close races. */
import assert from 'node:assert/strict';
import { DocumentServices, downloadDocumentBytes } from '../web/document_services.mjs';
import { AttachmentKey } from '../web/protocol_constants.mjs';

export async function testDocumentServicesView() {
  class Element {
    constructor(tag = 'div') { this.tagName = tag.toUpperCase(); }
    children = []; listeners = new Map(); textContent = ''; disabled = false; open = false;
    set innerHTML(_) { assert.fail('Document strings must never be interpreted as HTML'); }
    append(...items) { this.children.push(...items); }
    replaceChildren(...items) { this.children = items; }
    addEventListener(type, callback) { this.listeners.set(type, callback); }
    fire(type) { this.listeners.get(type)?.(); }
    showModal() { this.open = true; }
    close() { this.open = false; this.fire('close'); }
    focus() {}
    click() { this.clicked = true; }
    remove() { this.removed = true; }
  }
  const elements = new Map(), document = { createElement: tag => new Element(tag), body: new Element(), getElementById: id => {
    if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id);
  } };
  const hostile = '<img src="https://example.invalid/never-fetch" onerror="alert(1)">';
  const record = { index: 0, names: { UF: '../unsafe.html', F: null }, description: hostile, payloads: [
    { key: AttachmentKey.Unicode, declaredSize: 9007199254740993n }, { key: AttachmentKey.File, declaredSize: null }] };
  const downloads = [], selected = []; let reads = 0, finish, pending = false, fail = false, load;
  const client = { metadata: async () => { if (load) await load; return { info: { title: hostile }, language: 'de-DE', metadataObject: { number: 6, generation: 0 } }; },
    attachments: async () => [record], metadataBytes: async () => { reads++; return Uint8Array.of(1); },
    attachmentBytes: async (handle, index, key) => { reads++; selected.push({ handle, index, key }); if (pending) await new Promise(resolve => { finish = resolve; });
      if (fail) throw new Error('Password required'); return Uint8Array.of(255, 0); } };
  const state = { client, handle: null, epoch: 1 };
  const view = new DocumentServices({ state: () => state, document, download: (bytes, name) => downloads.push({ bytes, name }) });
  const nodes = root => [root, ...root.children.flatMap(nodes)];
  view.controls(); assert.equal(view.openButton.disabled, true); await view.show(); assert.equal(reads, 0);
  state.handle = 1; view.controls(); assert.equal(view.openButton.disabled, false); await view.show();
  assert.equal(reads, 0, 'listing cannot decode or download payloads');
  assert.ok(nodes(view.info).some(node => node.textContent === hostile));
  assert.ok(nodes(view.files).some(node => node.textContent === hostile));
  assert.ok(nodes(view.files).some(node => node.textContent === '../unsafe.html'));
  assert.ok(nodes(view.files).some(node => node.textContent.includes('9007199254740993 bytes')));
  const button = view.buttons[1]; button.fire('click'); await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(selected, [{ handle: 1, index: 0, key: AttachmentKey.Unicode }]);
  assert.equal(downloads[0].name, 'attachment-1-UF.bin', 'source path and extension cannot choose download destination');
  assert.deepEqual(downloads[0].bytes, Uint8Array.of(255, 0));
  fail = true; button.fire('click'); await Promise.resolve(); await Promise.resolve();
  assert.equal(downloads.length, 1); assert.match(view.message.textContent, /Password required/); assert.equal(button.disabled, false);
  fail = false; pending = true; button.fire('click'); const before = reads; button.fire('click'); assert.equal(reads, before, 'duplicate click is bounded');
  state.epoch++; state.handle = 2; view.reset(); finish(); await Promise.resolve(); await Promise.resolve();
  assert.equal(downloads.length, 1, 'old payload cannot download after opening another PDF');
  await view.show(); view.buttons[1].fire('click'); view.dialog.close(); finish(); await Promise.resolve(); await Promise.resolve();
  assert.equal(downloads.length, 1, 'closing the dialog cancels a pending download');
  let loaded; load = new Promise(resolve => { loaded = resolve; }); const showing = view.show();
  state.epoch++; view.reset(); loaded(); await showing;
  assert.equal(view.info.children.length, 0, 'stale metadata cannot repopulate a closed/new document dialog');
  load = null; pending = false; client.attachments = async () => { throw new Error('Malformed catalog'); };
  await view.show(); assert.ok(view.buttons.length > 0, 'attachment errors do not hide available metadata');
  assert.match(view.files.children[0].textContent, /Malformed catalog/);
  view.buttons[0].fire('click'); await Promise.resolve(); await Promise.resolve();
  assert.equal(downloads.at(-1).name, 'metadata.xmp');
  view.reset();
  checkDownload(document);
  console.log('Properties/attachments UI: inert source text, explicit selection, failed and stale downloads, exact declared sizes and URL cleanup passed.');
}

function checkDownload(document) {
  const original = { create: URL.createObjectURL, revoke: URL.revokeObjectURL, timeout: globalThis.setTimeout };
  let blob, release, revoked;
  URL.createObjectURL = value => { blob = value; return 'blob:local-payload'; };
  URL.revokeObjectURL = value => { revoked = value; };
  globalThis.setTimeout = callback => { release = callback; return 1; };
  try {
    downloadDocumentBytes(Uint8Array.of(0, 255), 'attachment-1-F.bin', document);
    assert.equal(blob.type, 'application/octet-stream'); assert.equal(blob.size, 2);
    const link = document.body.children.at(-1);
    assert.equal(link.href, 'blob:local-payload'); assert.equal(link.download, 'attachment-1-F.bin');
    assert.equal(link.clicked, true); assert.equal(link.removed, true);
    release(); assert.equal(revoked, 'blob:local-payload');
  } finally { URL.createObjectURL = original.create; URL.revokeObjectURL = original.revoke; globalThis.setTimeout = original.timeout; }
}
