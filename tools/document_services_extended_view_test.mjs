/** Explicit AF/RF source UI and attachment-only password retry/cleanup controls. */
import assert from 'node:assert/strict';
import { DocumentServices } from '../web/document_services.mjs';
import { FileScopeCode, ImageTransferCode, MetadataOwnerCode } from '../web/protocol_constants.mjs';

const tick = () => new Promise(resolve => setImmediate(resolve));
function dom() {
  class Element {
    children = []; listeners = new Map(); textContent = ''; value = ''; open = false; disabled = false;
    constructor(tag = 'div') { this.tagName = tag.toUpperCase(); }
    set innerHTML(_) { assert.fail('Source data must remain inert'); }
    append(...nodes) { this.children.push(...nodes); }
    replaceChildren(...nodes) { this.children = nodes; }
    addEventListener(event, fn) { this.listeners.set(event, fn); }
    fire(event) { this.listeners.get(event)?.({ preventDefault() {} }); }
    showModal() { this.open = true; }
    close() { this.open = false; this.fire('close'); }
    focus() {}
  }
  const elements = new Map(); return { createElement: tag => new Element(tag), getElementById: id => { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); } };
}
const all = node => [node, ...node.children.flatMap(all)];
const control = (view, label) => all(view.extended.output).find(node => node.tagName === 'BUTTON' && node.textContent === label);
const denied = () => Object.assign(new Error('Attachment password required'), { category: 6 });

export async function testExtendedServicesView() {
  const document = dom(), calls = [], downloads = [], passwords = [new TextEncoder().encode('wrong'), new TextEncoder().encode('good')];
  const record = { index: 0, names: { UF: '<script>file</script>', F: null }, description: '../never/open', payloads: [],
    related: [{ key: 0, index: 1, name: new TextEncoder().encode('same-name') }], collectionItem: [], encryptedPayload: null };
  let attempt = 0, hold = false, finish, cancel = 0;
  const client = { metadata: async () => ({ info: null, language: null, metadataObject: null }), attachments: async () => [],
    files: async (...args) => { calls.push(['files', ...args]); return [record]; },
    fileBytes: async (...args) => { calls.push(['bytes', ...args]); if (args[0] === 1) throw denied(); return Uint8Array.of(5, 0); },
    fileCheck: async () => ({ decodedSize: 2n, size: 1, checksum: 0 }),
    reauthenticate: async (handle, password) => { calls.push(['authenticate', handle, new TextDecoder().decode(password)]); if (hold) await new Promise(resolve => { finish = resolve; }); if (attempt++ === 0) throw denied(); return { document: 99, pages: 1 }; },
    close: async handle => { calls.push(['close', handle]); },
  };
  const state = { client, handle: 1, epoch: 1, count: 0 };
  const view = new DocumentServices({ state: () => state, document, download: (bytes, name) => downloads.push({ bytes, name }), password: async () => passwords.shift() ?? new Uint8Array([1]), cancel: () => { cancel++; } });
  await view.show(); view.extended.choice.value = 'associated'; await view.extended.show();
  assert.deepEqual(calls[0], ['files', 1, { kind: FileScopeCode.Associated }]);
  assert.ok(all(view.extended.output).some(node => node.textContent === '<script>file</script>'));
  const wrong = passwords[0], good = passwords[1]; control(view, 'Download related file 2').fire('click'); await tick();
  assert.deepEqual(downloads, [{ bytes: Uint8Array.of(5, 0), name: 'source-file-1-related-0-2.bin' }]);
  assert.ok(wrong.every(value => value === 0)); assert.ok(good.every(value => value === 0));
  assert.deepEqual(calls.filter(call => call[0] === 'authenticate'), [['authenticate', 1, 'wrong'], ['authenticate', 1, 'good']]);
  assert.deepEqual(calls.at(-1), ['close', 99]); assert.equal(state.handle, 1, 'unlock does not adopt or replace the edited source');
  control(view, 'Check related file 2').fire('click'); await tick(); assert.ok(all(view.extended.output).some(node => node.textContent.includes('2 decoded bytes')));
  hold = true; control(view, 'Download related file 2').fire('click'); await tick(); view.reset(); finish(); await tick();
  assert.equal(downloads.length, 1, 'stale authenticated handles cannot download'); assert.deepEqual(calls.at(-1), ['close', 99]); assert.ok(cancel > 0);
  hold = false; await view.show(); view.extended.choice.value = 'object-associated'; view.extended.object.value = '10:2'; await view.extended.show();
  assert.deepEqual(calls.at(-1), ['files', 1, { kind: FileScopeCode.ObjectAssociated, object: { number: 10, generation: 2 } }]);
  view.extended.object.value = '10:-1'; const before = calls.length; await view.extended.show(); assert.equal(calls.length, before); assert.match(view.extended.output.children[0].textContent, /number:generation/);
  view.reset();
  await sourcePanels();
  console.log('Extended service UI: exact AF/RF selection, inert text, password retry, temporary-handle cleanup and stale-operation refusal passed.');
}


async function sourcePanels() {
  const document = dom(), calls = [], downloads = [];
  const image = { object: { number: 9, generation: 2 }, width: 2, height: 1, thumbnail: false };
  const thumbnail = { object: { number: 12, generation: 0 }, width: 1, height: 1, thumbnail: true };
  const client = {
    metadata: async () => ({ info: null, language: null, metadataObject: null }), attachments: async () => [], pageAttachments: async () => [],
    metadataFor: async (...args) => { calls.push(['metadata', ...args]); return { number: 8, generation: 0 }; },
    metadataBytesFor: async (...args) => { calls.push(['packet', ...args]); return Uint8Array.of(60, 0, 62); },
    collection: async () => ({ view: 'Hidden', initialFile: new TextEncoder().encode('../inert'),
      fields: [{ name: '<script>label</script>', kind: 'Text', visible: true }], sort: [{ key: new TextEncoder().encode('label'), ascending: false }] }),
    images: async (...args) => { calls.push(['images', ...args]); return [image]; },
    thumbnail: async (...args) => { calls.push(['thumbnail', ...args]); return thumbnail; },
    imageBytes: async (...args) => { calls.push(['imageBytes', ...args]); return { bytes: Uint8Array.of(args[2], args[3] ? 1 : 0) }; },
  };
  const state = { client, handle: 7, epoch: 2, count: 2, page: 1 };
  const view = new DocumentServices({ state: () => state, document, download: (bytes, name) => downloads.push({ bytes, name }), cancel: () => {} });
  await view.show();
  view.extended.choice.value = 'page-metadata'; await view.extended.show();
  assert.deepEqual(calls.at(-1), ['metadata', 7, { kind: MetadataOwnerCode.Page, page: 1 }]);
  control(view, 'Download source metadata packet').fire('click'); await tick();
  assert.deepEqual(downloads.at(-1), { bytes: Uint8Array.of(60, 0, 62), name: 'page-2-metadata.xmp' });
  view.extended.choice.value = 'collection'; await view.extended.show();
  assert.ok(all(view.extended.output).some(node => node.textContent.includes('<script>label</script>')));
  assert.ok(all(view.extended.output).some(node => node.textContent.includes('descending')));
  view.extended.choice.value = 'images'; await view.extended.show();
  assert.deepEqual(calls.slice(-2), [
    ['images', 7, { kind: MetadataOwnerCode.Page, page: 1 }],
    ['thumbnail', 7, { kind: MetadataOwnerCode.Page, page: 1 }],
  ]);
  const sections = view.extended.output.children.filter(node => node.tagName === 'SECTION');
  assert.equal(sections.length, 2);
  const click = async (section, label) => { all(section).find(node => node.tagName === 'BUTTON' && node.textContent === label).fire('click'); await tick(); };
  await click(sections[0], 'Download stored stream');
  assert.deepEqual(calls.at(-1), ['imageBytes', 7, image.object, ImageTransferCode.Stored, false]);
  await click(sections[1], 'Download encoded asset');
  assert.deepEqual(calls.at(-1), ['imageBytes', 7, { page: 1 }, ImageTransferCode.Encoded, true]);
  assert.equal(downloads.at(-1).name, 'image-12-0-encoded.bin');
  view.extended.choice.value = 'thumbnail-object'; view.extended.object.value = '10:0'; await view.extended.show();
  assert.deepEqual(calls.at(-1), ['thumbnail', 7, { kind: MetadataOwnerCode.Object, object: { number: 10, generation: 0 } }]);
  control(view, 'Download encoded asset').fire('click'); await tick();
  assert.deepEqual(calls.at(-1), ['imageBytes', 7, { number: 10, generation: 0 }, ImageTransferCode.Encoded, true]);
  view.reset();
}
