/** Explicit AF/RF source UI and attachment-only password retry/cleanup controls. */
import assert from 'node:assert/strict';
import { DocumentServices } from '../web/document_services.mjs';
import { FileScopeCode } from '../web/protocol_constants.mjs';

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
  console.log('Extended service UI: exact AF/RF selection, inert text, password retry, temporary-handle cleanup and stale-operation refusal passed.');
}
