// Offline controls: documented public shapes, no PDF engine or dependencies.
import assert from 'node:assert/strict';
import { attachmentValues } from '../benchmarks/adapters/capability_pdfjs_services.mjs';

const bytes = new TextEncoder().encode('Synthetic attachment.\n');
const digest = 'bb0530979076ce983311c32a3cbf23448fb488520e3af66529a30246dbf01680';
const requested = [];
assert.deepEqual(await attachmentValues({
  getAttachments: async () => new Map([
    ['lazy', { filename: 'z.txt' }],
    ['ready', { filename: 'a.txt', content: bytes }],
  ]),
  getAttachmentContent: async id => { requested.push(id); return bytes; },
}), [{ name: 'a.txt', sha256: digest }, { name: 'z.txt', sha256: digest }]);
assert.deepEqual(requested, ['lazy']);
for (const empty of [null, new Map()]) {
  assert.deepEqual(await attachmentValues({ getAttachments: async () => empty }), []);
}
await assert.rejects(attachmentValues({ getAttachments: async () => ({}) }), /attachment Map/);
await assert.rejects(attachmentValues({
  getAttachments: async () => new Map([['id', { filename: 'lost.txt' }]]),
  getAttachmentContent: async () => null,
}), /missing attachment bytes/);
console.log('pdf.js public attachment API controls passed (no engine invoked).');
