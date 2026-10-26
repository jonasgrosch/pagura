/** Viewer page selection must preserve scope and discard obsolete operations. */
import assert from 'node:assert/strict';
import { DocumentServices } from '../web/document_services.mjs';

function dom() {
  class Element {
    constructor(tag = 'div') { this.tagName = tag.toUpperCase(); }
    children = []; listeners = new Map(); textContent = ''; value = ''; disabled = false; open = false;
    set innerHTML(_) { assert.fail('Source text must remain inert'); }
    append(...items) { this.children.push(...items); }
    replaceChildren(...items) { this.children = items; }
    addEventListener(type, callback) { this.listeners.set(type, callback); }
    fire(type) { this.listeners.get(type)?.({ preventDefault() {} }); }
    showModal() { this.open = true; }
    close() { this.open = false; this.fire('close'); }
    focus() {}
  }
  const elements = new Map();
  return { createElement: tag => new Element(tag), getElementById: id => {
    if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id);
  } };
}

const nodes = root => [root, ...root.children.flatMap(nodes)];
const button = (root, label) => nodes(root).find(node => node.tagName === 'BUTTON' && node.textContent === label);
const tick = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };

export async function testPageAttachmentView() {
  const document = dom(), calls = [], downloads = [];
  let failList = false, failRead = false, holdList = false, holdRead = false, finishList, finishRead;
  const payload = { key: 1, declaredSize: 9007199254740993n };
  const file = { names: { UF: '<img src="https://example.invalid/never">' }, description: 'Separate file description', payloads: [payload] };
  const catalog = { index: 0, names: { UF: 'Catalog file' }, description: null, payloads: [payload] };
  const records = [{ index: 1, contents: '<script>note</script>', icon: null, file },
    { index: 2, contents: '', icon: Uint8Array.of(255), file }];
  const client = {
    metadata: async () => ({ info: null, language: null, metadataObject: null }),
    attachments: async () => [catalog],
    pageAttachments: async (...args) => {
      calls.push(['list', ...args]);
      if (holdList) await new Promise(resolve => { finishList = resolve; });
      if (failList) throw new Error('Malformed page annotations'); return records;
    },
    attachmentBytes: async (...args) => { calls.push(['catalog-bytes', ...args]); return Uint8Array.of(9); },
    pageAttachmentBytes: async (...args) => {
      calls.push(['bytes', ...args]);
      if (holdRead) await new Promise(resolve => { finishRead = resolve; });
      if (failRead) throw new Error('Attachment password required'); return Uint8Array.of(1, 2);
    },
    pageAttachmentCheck: async (...args) => {
      calls.push(['check', ...args]);
      if (holdRead) await new Promise(resolve => { finishRead = resolve; });
      if (failRead) throw new Error('Attachment password required');
      return { decodedSize: 9007199254740993n, size: 0, checksum: 2 };
    },
  };
  const state = { client, handle: 3, epoch: 1, page: 1, count: 3 };
  const view = new DocumentServices({ state: () => state, document, download: (bytes, name) => downloads.push({ bytes, name }) });
  await view.show();
  assert.deepEqual(calls, [['list', 3, 1]], 'opening lists only the current page and does not decode payloads');
  assert.equal(view.pageInput.value, '2'); assert.equal(view.pageInput.max, '3');
  assert.equal(view.pageTitle.textContent, 'Page 2 attachments');
  const text = nodes(view.pageFiles).map(node => node.textContent);
  for (const value of [file.names.UF, 'Annotation note: <script>note</script>', 'File description: Separate file description', 'Icon: PushPin (default)', 'Icon: Bytes: ff']) assert.ok(text.includes(value), value);
  assert.ok(text.some(value => value.includes('9007199254740993 bytes')));
  const oldDownload = button(view.pageFiles, 'Download attachment'); oldDownload.fire('click'); await tick();
  assert.deepEqual(calls.at(-1), ['bytes', 3, 1, 1, 1]);
  assert.equal(downloads[0].name, 'page-2-attachment-2-UF.bin'); assert.deepEqual(downloads[0].bytes, Uint8Array.of(1, 2));
  button(view.pageFiles, 'Check size and checksum').fire('click'); await tick();
  assert.deepEqual(calls.at(-1), ['check', 3, 1, 1, 1]); assert.equal(downloads.length, 1);
  assert.ok(nodes(view.pageFiles).some(node => node.textContent.includes('9007199254740993 decoded bytes. Declared size: not declared. Declared checksum: does not match.')));
  view.pageInput.value = '1'; await view.selectPage();
  assert.deepEqual(calls.at(-1), ['list', 3, 0]); const before = calls.length;
  oldDownload.fire('click'); await tick(); assert.equal(calls.length, before, 'stale entry callbacks cannot select an old page');
  for (const invalid of ['', '0', '4', '1.5']) {
    view.pageInput.value = invalid; await view.selectPage(); assert.equal(calls.length, before); assert.match(view.message.textContent, /Choose a page/);
  }
  failList = true; view.pageInput.value = '3'; await view.selectPage();
  assert.match(view.pageFiles.children[0].textContent, /Malformed page annotations/);
  assert.ok(button(view.files, 'Download attachment'), 'a page error preserves independent catalog downloads');
  button(view.files, 'Download attachment').fire('click'); await tick(); assert.deepEqual(calls.at(-1), ['catalog-bytes', 3, 0, 1]);
  failList = false; await view.show(); failRead = true;
  button(view.pageFiles, 'Download attachment').fire('click'); await tick(); assert.match(view.message.textContent, /password required/);
  button(view.pageFiles, 'Check size and checksum').fire('click'); await tick(); assert.match(view.message.textContent, /check failed/);
  assert.equal(downloads.length, 2); failRead = false;

  // Both decode operations share the same immutable page/document/dialog guard.
  for (const label of ['Download attachment', 'Check size and checksum']) {
    for (const change of ['page', 'handle', 'client', 'epoch', 'close']) {
      await view.show(); const control = button(view.pageFiles, label); holdRead = true;
      const output = view.pageFiles.children[0]; control.fire('click'); const count = calls.length;
      control.fire('click'); assert.equal(calls.length, count, 'duplicate operations are suppressed');
      if (change === 'page') { holdRead = false; view.pageInput.value = '1'; await view.selectPage(); }
      if (change === 'handle') state.handle++;
      if (change === 'client') state.client = { ...client };
      if (change === 'epoch') state.epoch++;
      if (change === 'close') view.dialog.close();
      const message = view.message.textContent; finishRead(); await tick(); holdRead = false;
      assert.equal(downloads.length, 2, `stale ${change} cannot download`);
      assert.equal(view.message.textContent, message, `stale ${change} cannot alter current status`);
      assert.ok(!nodes(output).some(node => node.textContent.includes('decoded bytes.')), 'stale checks cannot display completed observations');
      view.reset(); state.client = client;
    }
  }
  holdList = true; const loading = view.show(); const listed = calls.length;
  await view.selectPage(); assert.equal(calls.length, listed, 'the same in-flight selection is not duplicated');
  holdList = false; view.pageInput.value = '1'; await view.selectPage();
  const current = view.pageFiles.children[0]; finishList(); await loading;
  assert.equal(view.pageFiles.children[0], current, 'an older page listing cannot replace the selected page');
  holdList = true; const closing = view.show(); view.reset(); finishList(); await closing;
  assert.equal(view.pageFiles.children.length, 0, 'closed dialog remains clear'); holdList = false;
  state.count = 0; const last = calls.length; await view.show();
  assert.equal(calls.length, last); assert.equal(view.pageSubmit.disabled, true); assert.match(view.pageFiles.children[0].textContent, /no pages/);
  view.reset();
  console.log('Page attachment UI: explicit selection, inert labels, exact slots, failure isolation and stale listing/decode suppression passed.');
}
