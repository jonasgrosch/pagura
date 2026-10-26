/** Public page attachment operations: independent fixture values and native/WASI/client equality. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { decodeResponse, encodeRequest, FrameDecoder, VERSION_1_11 } from '../web/protocol.mjs';
import { HostFailure } from '../web/failure.mjs';

const utf8 = value => new TextEncoder().encode(value);
const payload = Uint8Array.from([...utf8('attachment\0bytes'), 255, 10]);
function parse(bytes) { const frames = [], decoder = new FrameDecoder(frame => frames.push(frame)); decoder.feed(bytes); decoder.finish(); return frames; }

export async function testPageAttachments(client, root, native, wasiRun, makeClient) {
  const fixture = async name => new Uint8Array(await readFile(join(root, 'corpus/generated/file_annotations', `${name}.pdf`)));
  const file = { object: { number: 5, generation: 0 }, names: { UF: 'Grüße Ω.txt', F: utf8('legacy.txt'), DOS: null, Mac: null, Unix: null },
    description: 'Attachment €', fileSystem: null, relationship: utf8('Data'), volatile: true,
    payloads: [0, 1].map(key => ({ key, object: { number: 6, generation: 0 }, mediaType: utf8('application/octet-stream'), declaredSize: 18n })) };
  for (const [name, slots, contents, icon, object] of [
    ['populated', [0], 'Annotation Ω', utf8('Paperclip'), 8],
    ['page-only', [0], null, null, 8],
    ['mixed', [1, 2], '', utf8('CustomIcon'), 9],
  ]) {
    const bytes = await fixture(name), expected = slots.map(index => ({ index, object: { number: object, generation: 0 },
      rectangle: [1, 2, 9, 12], contents, icon, file }));
    const selection = { document: 1, page: 0, index: slots[0], key: 1 };
    const input = Buffer.concat([encodeRequest(1, 1, { version: VERSION_1_11 }),
      encodeRequest(2, 2, { bytes, recovery: 0, password: new Uint8Array() }), encodeRequest(28, 3, { document: 1, page: 0 }),
      encodeRequest(29, 4, selection), encodeRequest(30, 5, selection)]);
    const result = spawnSync(native, [], { input, timeout: 30000 }); assert.ifError(result.error); assert.equal(result.status, 0);
    const wasi = await wasiRun(input, `page-attachments-${name}`); assert.equal(wasi.code, 0);
    assert.deepEqual(wasi.bytes, result.stdout, 'page attachments native/WASI wire identity');
    const frames = parse(wasi.bytes), checked = { object: { number: 6, generation: 0 }, decodedSize: 18n, size: 1, checksum: 1 };
    assert.deepEqual(frames[2].value, expected); assert.deepEqual(frames[3].value, payload); assert.deepEqual(frames[4].value, checked);
    const opened = await client.open(bytes, 0);
    assert.deepEqual(await client.pageAttachments(opened.document, 0), expected);
    for (const index of slots) {
      assert.deepEqual(await client.pageAttachmentBytes(opened.document, 0, index, 1), payload);
      assert.deepEqual(await client.pageAttachmentCheck(opened.document, 0, index, 1), checked);
    }
    if (name === 'mixed') {
      await assert.rejects(client.pageAttachmentBytes(opened.document, 0, 0, 0), /Annots slot/);
      await assert.rejects(client.pageAttachmentCheck(opened.document, 0, 0, 0), /Annots slot/);
      assert.equal((await client.attachments(opened.document)).length, 1, 'catalog index zero exists independently');
    }
    if (name === 'page-only') assert.deepEqual(await client.attachments(opened.document), []);
    await assert.rejects(client.pageAttachments(opened.document, 1));
    await assert.rejects(client.pageAttachmentBytes(opened.document, 0, slots[0], 4), /no payload/);
    await client.close(opened.document);
  }
  for (const name of ['absent', 'empty', 'missing', 'external', 'late-failure', 'flate']) {
    const opened = await client.open(await fixture(name), 0);
    if (name === 'late-failure') await assert.rejects(client.pageAttachments(opened.document, 0));
    else if (name === 'external') {
      const records = await client.pageAttachments(opened.document, 0);
      assert.deepEqual(records[0].file.payloads, []);
      assert.deepEqual(records[0].file.names.F, utf8('../outside.bin'));
      await assert.rejects(client.pageAttachmentBytes(opened.document, 0, 0, 0));
    } else if (name === 'flate') {
      assert.deepEqual(await client.pageAttachmentBytes(opened.document, 0, 0, 1), payload);
      assert.deepEqual(await client.pageAttachmentCheck(opened.document, 0, 0, 1), {
        object: { number: 6, generation: 0 }, decodedSize: 18n, size: 0, checksum: 0 });
    } else assert.deepEqual(await client.pageAttachments(opened.document, 0), []);
    await client.pageInfo(opened.document, 0); await client.close(opened.document);
  }
  for (const name of ['encrypted-no-type', 'encrypted-identity']) {
    for (const password of ['', 'wrong', 'attachment', 'owner']) {
      const opened = await client.open(await fixture(name), 1, utf8(password));
      assert.equal((await client.pageAttachments(opened.document, 0)).length, 1, 'discovery is lazy');
      if (name === 'encrypted-identity' || ['attachment', 'owner'].includes(password)) {
        const bytes = await client.pageAttachmentBytes(opened.document, 0, 0, 0);
        const checked = await client.pageAttachmentCheck(opened.document, 0, 0, 0);
        assert.equal(checked.decodedSize, BigInt(bytes.length)); assert.equal(checked.size, 0); assert.equal(checked.checksum, 0);
      } else {
        await assert.rejects(client.pageAttachmentBytes(opened.document, 0, 0, 0), error => error.category === 6);
        await assert.rejects(client.pageAttachmentCheck(opened.document, 0, 0, 0), error => error.category === 6);
      }
      await client.pageInfo(opened.document, 0); await client.close(opened.document);
    }
  }
  const limited = await fixture('listing-limit');
  const input = Buffer.concat([encodeRequest(1, 1, { version: VERSION_1_11 }),
    encodeRequest(2, 2, { bytes: limited, recovery: 0, password: new Uint8Array() }),
    encodeRequest(28, 3, { document: 1, page: 0 }), encodeRequest(3, 4, { document: 1, page: 0 })]);
  const result = spawnSync(native, [], { input, timeout: 30000 }); assert.ifError(result.error); assert.equal(result.status, 0);
  const wasi = await wasiRun(input, 'page-attachments-listing-limit'); assert.equal(wasi.code, 0);
  assert.deepEqual(wasi.bytes, result.stdout, 'listing refusal native/WASI wire identity');
  const frames = parse(wasi.bytes); assert.equal(frames[2].value.category, 2);
  assert.match(frames[2].value.message, /page attachment listing budget exceeded/);
  assert.equal(frames[3].kind, 131, 'the native/WASI protocol retains the document after this refusal');
  // Browser resource-failure policy is deliberately stronger than wire-level
  // continuation: isolate this client, assert termination, then discard it.
  const bounded = makeClient();
  try {
    await bounded.hello(); const opened = await bounded.open(limited, 0);
    await assert.rejects(bounded.pageAttachments(opened.document, 0), error => error instanceof HostFailure && error.reason === 'budget exceeded');
    assert.ok(bounded.failure instanceof HostFailure);
    await assert.rejects(bounded.pageInfo(opened.document, 0), error => error === bounded.failure);
  } finally { bounded.destroy(); }
  checkCodec();
  console.log('Page attachments: exact values, scope, budgets/authentication, canonical codec and native/WASI/client parity passed.');
}

function checkCodec() {
  assert.deepEqual([...encodeRequest(28, 1, { document: 2, page: 3 })], [13, 0, 0, 0, 28, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]);
  for (const kind of [29, 30]) assert.deepEqual([...encodeRequest(kind, 1, { document: 2, page: 3, index: 4, key: 1 })],
    [18, 0, 0, 0, kind, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 1]);
  const listing = new Uint8Array(67), view = new DataView(listing.buffer);
  listing[0] = 156; listing[1] = 1; listing[5] = 1; listing[13] = 8;
  [1, 2, 9, 12].forEach((value, index) => view.setFloat64(19 + index * 8, value, true));
  const check = new Uint8Array(21), checked = new DataView(check.buffer);
  check[0] = 158; check[1] = 1; checked.setBigUint64(11, 9007199254740993n, true); check[20] = 2;
  assert.equal(decodeResponse(check).value.decodedSize, 9007199254740993n);
  for (const bytes of [listing, check, Uint8Array.of(157, 1, 0, 0, 0, 0, 0, 0, 0)]) {
    assert.doesNotThrow(() => decodeResponse(bytes));
    for (let end = 0; end < bytes.length; end++) assert.throws(() => decodeResponse(bytes.subarray(0, end)));
    assert.throws(() => decodeResponse(Uint8Array.from([...bytes, 0])), /Trailing/);
  }
  for (const value of [Infinity, NaN, 10]) {
    const bad = listing.slice(); new DataView(bad.buffer).setFloat64(19, value, true);
    assert.throws(() => decodeResponse(bad), /rectangle/);
  }
  const duplicate = new Uint8Array(125); duplicate.set(listing); duplicate[5] = 2; duplicate.set(listing.subarray(9), 67);
  assert.throws(() => decodeResponse(duplicate), /indexes/);
  for (const index of [51, 52, 53, 54]) {
    const bad = listing.slice(); bad[index] = 2; assert.throws(() => decodeResponse(bad), /flag/);
  }
  for (const status of [3, 255]) {
    const bad = check.slice(); bad[19] = status; assert.throws(() => decodeResponse(bad), /status/);
  }
  for (const field of ['document', 'page', 'index']) {
    for (const bad of [-1, 1.5, true, 0x100000000]) assert.throws(() => encodeRequest(29, 1, { document: 1, page: 0, index: 0, key: 0, [field]: bad }), /u32/);
  }
  for (const key of [-1, 5, 256, '1']) assert.throws(() => encodeRequest(30, 1, { document: 1, page: 0, index: 0, key }), /key/);
}
