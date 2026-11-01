// Offline controls: documented public shapes, no PDF engine or dependencies.
import assert from 'node:assert/strict';
import { attachmentValues, annotationAttachmentValues, ProbeLimit } from '../benchmarks/adapters/capability_pdfjs_services.mjs';

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
const fileType = 17, annotationRequests = [];
const fake = annotations => ({
  getPage: async page => {
    assert.equal(page, 1);
    return { getAnnotations: async options => {
      assert.deepEqual(options, { intent: 'any' }); return annotations;
    } };
  },
  getAttachmentContent: async id => { annotationRequests.push(id); return bytes; },
  getAttachments: () => { throw new Error('page selection must not fall back to catalog'); },
});
assert.deepEqual(await annotationAttachmentValues(fake([
  { annotationType: 2 },
  { annotationType: fileType, fileId: 'shared', file: { filename: 'probe.txt' } },
  { annotationType: fileType, fileId: 'shared', file: { filename: 'probe.txt' } },
]), fileType), [{ name: 'probe.txt', sha256: digest }, { name: 'probe.txt', sha256: digest }]);
assert.deepEqual(annotationRequests, ['shared', 'shared']);
assert.deepEqual(await annotationAttachmentValues(fake([]), fileType), []);
await assert.rejects(annotationAttachmentValues(fake(null), fileType), /annotation array/);
await assert.rejects(annotationAttachmentValues(fake([{ annotationType: fileType, file: { filename: 'x' } }]), fileType), /descriptor/);
await assert.rejects(annotationAttachmentValues({ ...fake([
  { annotationType: fileType, fileId: 'lost', file: { filename: 'x' } },
]), getAttachmentContent: async () => null }, fileType), /missing attachment bytes/);
await assert.rejects(annotationAttachmentValues(fake(Array(1025).fill({ annotationType: 2 })), fileType), ProbeLimit);
console.log('pdf.js catalog/page attachment API controls passed (no engine invoked).');
