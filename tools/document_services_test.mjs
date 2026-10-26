/** Hosted native/WASI/client evidence using independently authored document fixtures. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { decodeResponse, encodeRequest, FrameDecoder, VERSION_1_8 } from '../web/protocol.mjs';
import { AttachmentKey } from '../web/protocol_constants.mjs';

const packet = new TextEncoder().encode('<?xpacket begin=""?><x:xmpmeta xmlns:x="adobe:ns:meta/"><title>XMP title</title></x:xmpmeta><?xpacket end="w"?>');
const payload = Uint8Array.from([...new TextEncoder().encode('attachment\0bytes'), 255, 10]);
function parse(bytes) { const frames = [], decoder = new FrameDecoder(frame => frames.push(frame)); decoder.feed(bytes); decoder.finish(); return frames; }

export async function testDocumentServices(client, root, native, wasiRun) {
  const fixture = async (area, name) => new Uint8Array(await readFile(join(root, 'corpus/generated', area, `${name}.pdf`)));
  for (const name of ['populated', 'populated-flate']) {
    const bytes = await fixture('metadata', name);
    const input = Buffer.concat([encodeRequest(1, 1, { version: VERSION_1_8 }), encodeRequest(2, 2, { bytes, recovery: 0, password: new Uint8Array() }),
      encodeRequest(22, 3, { document: 1 }), encodeRequest(24, 4, { document: 1 })]);
    const result = spawnSync(native, [], { input, timeout: 30000 }); assert.ifError(result.error); assert.equal(result.status, 0);
    const wasi = await wasiRun(input, `document-${name}`); assert.equal(wasi.code, 0);
    assert.deepEqual(wasi.bytes, result.stdout, 'metadata native/WASI wire identity');
    const frames = parse(wasi.bytes), expected = { info: { object: { number: 5, generation: 0 }, title: 'Grüße Ω', author: 'Euro €', subject: '測試',
      keywords: 'one; two', creator: 'Original app', producer: 'PDF app', creationDate: "D:20261026030000+01'00", modificationDate: 'D:20261026040000Z', trapped: 'True' },
      language: 'de-DE', metadataObject: { number: 6, generation: 0 } };
    assert.deepEqual(frames[2].value, expected); assert.deepEqual(frames[3].value, packet);
    const opened = await client.open(bytes, 0); assert.deepEqual(await client.metadata(opened.document), expected);
    assert.deepEqual(await client.metadataBytes(opened.document), packet); await client.close(opened.document);
  }
  for (const name of ['absent', 'null', 'empty']) {
    const opened = await client.open(await fixture('metadata', name), 0), value = await client.metadata(opened.document);
    assert.equal(value.language, null);
    assert.deepEqual(await client.metadataBytes(opened.document), name === 'empty' ? new Uint8Array() : null);
    if (name === 'empty') { assert.equal(value.info.title, null); assert.equal(value.info.trapped, 'Unknown'); }
    else assert.equal(value.info, null);
    await client.close(opened.document);
  }
  const inert = await client.open(await fixture('metadata', 'metadata-inert-xml'), 0);
  assert.equal(new TextDecoder().decode(await client.metadataBytes(inert.document)), '<!DOCTYPE x [<!ENTITY e SYSTEM "https://example.invalid/metadata">]><x>&e;</x>');
  await client.close(inert.document);
  for (const name of ['info-wrong-type', 'info-field-wrong-type', 'info-cycle', 'invalid-text', 'language-wrong-type', 'metadata-cycle']) {
    const opened = await client.open(await fixture('metadata', name), 0);
    await assert.rejects(client.metadata(opened.document)); await client.pageInfo(opened.document, 0); await client.close(opened.document);
  }
  const bytes = await fixture('files', 'populated'), opened = await client.open(bytes, 0);
  const records = await client.attachments(opened.document);
  assert.equal(records.length, 1);
  assert.deepEqual(records[0], { index: 0, key: new TextEncoder().encode('lookup-key'), object: { number: 5, generation: 0 },
    names: { UF: 'Grüße Ω.txt', F: new TextEncoder().encode('legacy.txt'), DOS: null, Mac: null, Unix: null }, description: 'Attachment €', fileSystem: null,
    relationship: new TextEncoder().encode('Data'), volatile: true, payloads: [AttachmentKey.File, AttachmentKey.Unicode].map(key => ({ key,
      object: { number: 6, generation: 0 }, mediaType: new TextEncoder().encode('application/octet-stream'), declaredSize: 18n })) });
  const input = Buffer.concat([encodeRequest(1, 1, { version: VERSION_1_8 }), encodeRequest(2, 2, { bytes, recovery: 0, password: new Uint8Array() }),
    encodeRequest(23, 3, { document: 1 }), encodeRequest(25, 4, { document: 1, index: 0, key: AttachmentKey.Unicode })]);
  const result = spawnSync(native, [], { input, timeout: 30000 }); assert.ifError(result.error); assert.equal(result.status, 0);
  const wasi = await wasiRun(input, 'document-attachments'); assert.equal(wasi.code, 0);
  assert.deepEqual(wasi.bytes, result.stdout, 'attachment native/WASI wire identity');
  assert.deepEqual(parse(wasi.bytes)[2].value, records); assert.deepEqual(parse(wasi.bytes)[3].value, payload);
  await assert.rejects(client.attachmentBytes(opened.document, 1, AttachmentKey.File), /index outside/);
  await assert.rejects(client.attachmentBytes(opened.document, 0, AttachmentKey.Unix), /no payload/);
  assert.deepEqual(await client.attachmentBytes(opened.document, 0, AttachmentKey.Unicode), payload); await client.close(opened.document);
  const largeSize = await client.open(await fixture('files', 'large-size-declaration'), 0);
  assert.equal((await client.attachments(largeSize.document))[0].payloads[0].declaredSize, 9007199254740993n);
  assert.deepEqual(await client.attachmentBytes(largeSize.document, 0, AttachmentKey.File), payload, 'declared Size is not an allocation hint or verified payload length');
  await client.close(largeSize.document);
  for (const name of ['external-url', 'simple-string', 'empty-unicode-name', 'absent', 'duplicate-payload', 'invalid-unicode']) {
    const view = await client.open(await fixture('files', name), 0);
    if (name === 'invalid-unicode') await assert.rejects(client.attachments(view.document));
    else {
      const files = await client.attachments(view.document);
      if (name === 'absent') assert.deepEqual(files, []);
      if (name === 'empty-unicode-name') assert.equal(files[0].names.UF, '');
      if (name === 'simple-string') assert.deepEqual(files[0].names.F, Uint8Array.from([47, 114, 97, 119, 255, 47, 102, 105, 108, 101]));
      if (name === 'external-url') { assert.equal(new TextDecoder().decode(files[0].fileSystem), 'URL'); assert.deepEqual(files[0].payloads, []); await assert.rejects(client.attachmentBytes(view.document, 0, AttachmentKey.File)); }
      if (name === 'duplicate-payload') assert.deepEqual(files.map(file => file.index), [0, 1]);
    }
    await client.close(view.document);
  }
  checkCodecBounds();
  console.log('Document services: exact metadata/payloads, native/WASI/client parity, inert external data and explicit attachment selection passed.');
}

function checkCodecBounds() {
  for (const bytes of [Uint8Array.of(150, 1, 0, 0, 0, 0, 0, 0), Uint8Array.of(152, 1, 0, 0, 0, 1, 0, 0, 0, 0), Uint8Array.of(153, 1, 0, 0, 0, 0, 0, 0, 0)]) {
    assert.doesNotThrow(() => decodeResponse(bytes));
    for (let i = 0; i < bytes.length; i++) assert.throws(() => decodeResponse(bytes.subarray(0, i)));
    assert.throws(() => decodeResponse(Uint8Array.from([...bytes, 0])), /Trailing/);
  }
  assert.throws(() => decodeResponse(Uint8Array.of(152, 1, 0, 0, 0, 2)), /flag/);
  assert.throws(() => decodeResponse(Uint8Array.of(151, 1, 0, 0, 0, 255, 255, 255, 255)), /count/);
  for (const key of [-1, 5, 256, 1.5, '1']) assert.throws(() => encodeRequest(25, 1, { document: 1, index: 0, key }), /key/);
  assert.throws(() => encodeRequest(25, 1, { document: 1, index: -1, key: 0 }), /u32/);
}
