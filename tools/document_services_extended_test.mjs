/** Hosted public/native/WASI/client contract; expected bytes come from the fixture author. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { encodeRequest, FrameDecoder, VERSION_1_12 } from '../web/protocol.mjs';
import { FileScopeCode, MetadataOwnerCode, ImageTransferCode, RequestKind } from '../web/protocol_constants.mjs';

const bytes = text => new TextEncoder().encode(text);
const parse = bytes => { const output = [], decoder = new FrameDecoder(frame => output.push(frame)); decoder.feed(bytes); decoder.finish(); return output; };

export async function testExtendedServices(client, root, native, wasiRun) {
  const pdf = new Uint8Array(await readFile(join(root, 'corpus/generated/document_services/complete.pdf')));
  const catalog = { kind: FileScopeCode.Catalog }, page = { kind: MetadataOwnerCode.Page, page: 0 }, object = { kind: MetadataOwnerCode.Object, object: { number: 10, generation: 0 } };
  const requests = [
    [RequestKind.Files, { document: 1, scope: catalog }],
    [RequestKind.FileBytes, { document: 1, scope: { kind: FileScopeCode.PageAssociated, page: 0 }, index: 1, key: 0, related: 1 }],
    [RequestKind.Collection, { document: 1 }], [RequestKind.ObjectMetadataBytes, { document: 1, owner: object }],
    [RequestKind.Images, { document: 1, owner: page }], [RequestKind.Thumbnail, { document: 1, owner: page }],
    [RequestKind.ImageBytes, { document: 1, owner: page, mode: ImageTransferCode.Encoded, thumbnail: true }],
  ];
  const input = Buffer.concat([encodeRequest(1, 1, { version: VERSION_1_12 }), encodeRequest(2, 2, { bytes: pdf, password: new Uint8Array(), recovery: 0 }),
    ...requests.map(([kind, values], index) => encodeRequest(kind, index + 3, values))]);
  const result = spawnSync(native, [], { input, timeout: 30000 }); assert.ifError(result.error); assert.equal(result.status, 0);
  const wasi = await wasiRun(input, 'extended-document-services'); assert.equal(wasi.code, 0); assert.deepEqual(wasi.bytes, result.stdout);
  const frames = parse(wasi.bytes); for (const frame of frames) assert.notEqual(frame.kind, 255, String(frame.value));
  const file = frames[2].value[0]; assert.equal(file.index, 0); assert.equal(file.related.length, 2);
  assert.deepEqual(file.related.map(value => [value.index, new TextDecoder().decode(value.name)]), [[0, 'same-name'], [1, 'same-name']]);
  assert.equal(file.collectionItem.find(item => new TextDecoder().decode(item.key) === 'amount').value, 9007199254740993n);
  assert.deepEqual(frames[3].value, bytes('second sidecar\0'));
  assert.equal(frames[4].value.view, 'Hidden'); assert.deepEqual(frames[4].value.sort.map(sort => sort.ascending), [false, true, true]);
  assert.deepEqual(frames[5].value, bytes('<x:xmpmeta xmlns:x="adobe:ns:meta/">object packet</x:xmpmeta>'));
  assert.equal(frames[6].value.length, 2); assert.equal(frames[7].value.thumbnail, true);
  assert.deepEqual(frames[8].value, { mode: 'encoded', codec: null, bytes: Uint8Array.of(127) });
  const opened = await client.open(pdf, 0);
  assert.deepEqual(await client.files(opened.document, catalog), frames[2].value);
  assert.deepEqual(await client.fileBytes(opened.document, catalog, 0, 0, 1), bytes('second sidecar\0'));
  await assert.rejects(client.fileBytes(opened.document, catalog, 0, 0, 2), /no payload/);
  assert.deepEqual(await client.collection(opened.document), frames[4].value);
  assert.deepEqual(await client.metadataBytesFor(opened.document, object), frames[5].value);
  assert.deepEqual(await client.images(opened.document, page), frames[6].value);
  assert.deepEqual(await client.thumbnail(opened.document, page), frames[7].value);
  assert.deepEqual(await client.imageBytes(opened.document, { page: 0 }, ImageTransferCode.Encoded, true), frames[8].value);
  await client.close(opened.document);
  const encrypted = new Uint8Array(await readFile(join(root, 'corpus/generated/files/encrypted-no-type.pdf'))), original = await client.open(encrypted, 1);
  await assert.rejects(client.fileBytes(original.document, catalog, 0, 0), error => error.category === 6);
  await assert.rejects(client.reauthenticate(original.document, bytes('wrong')), error => error.category === 6);
  const authenticated = await client.reauthenticate(original.document, bytes('attachment'));
  assert.deepEqual(await client.fileBytes(authenticated.document, catalog, 0, 0), bytes('attached file bytes'));
  await assert.rejects(client.fileBytes(original.document, catalog, 0, 0), error => error.category === 6);
  await client.close(authenticated.document); await client.close(original.document);
  console.log('Extended document services: independent bytes, scoped RF/AF, collections, metadata, thumbnails and isolated reauthentication passed.');
}
