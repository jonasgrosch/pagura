/** Attachment checks use independent fixture observations and literal wire expectations. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { decodeResponse, encodeRequest, FrameDecoder, VERSION_1_10 } from '../web/protocol.mjs';
import { DocumentServices } from '../web/document_services.mjs';

function frames(bytes) { const result = [], decoder = new FrameDecoder(frame => result.push(frame)); decoder.feed(bytes); decoder.finish(); return result; }

export async function testAttachmentChecks(client, root, native, wasiRun) {
  for (const [name, size, checksum] of [
    ['check-flate', 1, 1], ['check-size-mismatch', 2, 1], ['check-checksum-mismatch', 1, 2],
    ['check-both-mismatch', 2, 2], ['check-size-only', 1, 0], ['check-checksum-only', 0, 1], ['check-empty-params', 0, 0],
  ]) {
    const bytes = new Uint8Array(await readFile(join(root, 'corpus/generated/files', `${name}.pdf`)));
    const input = Buffer.concat([encodeRequest(1, 1, { version: VERSION_1_10 }),
      encodeRequest(2, 2, { bytes, recovery: 0, password: new Uint8Array() }),
      encodeRequest(27, 3, { document: 1, index: 0, key: 1 })]);
    const result = spawnSync(native, [], { input, timeout: 30000 }); assert.ifError(result.error); assert.equal(result.status, 0);
    const wasi = await wasiRun(input, `attachment-${name}`); assert.equal(wasi.code, 0);
    assert.deepEqual(wasi.bytes, result.stdout, `${name}: native/WASI wire identity`);
    const expected = { object: { number: 6, generation: 0 }, decodedSize: 18n, size, checksum };
    assert.deepEqual(frames(wasi.bytes)[2].value, expected, name);
    const opened = await client.open(bytes, 0);
    try {
      assert.deepEqual(await client.attachmentCheck(opened.document, 0, 1), expected, `${name}: client observation`);
      await assert.rejects(client.attachmentCheck(opened.document, 1, 1), /index outside/);
      await assert.rejects(client.attachmentCheck(opened.document, 0, 4), /no payload/);
      await client.pageInfo(opened.document, 0);
    } finally { await client.close(opened.document); }
  }
  codecChecks();
  console.log('Attachment checks: independent missing/match/mismatch outcomes, native/WASI/client identity and exact selection passed.');
}

function codecChecks() {
  const request = encodeRequest(27, 3, { document: 1, index: 0, key: 1 });
  assert.deepEqual([...request], [14, 0, 0, 0, 27, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1]);
  for (const values of [{ document: -1, index: 0, key: 1 }, { document: 1, index: 0x100000000, key: 1 },
    { document: 1, index: 0, key: 5 }, { document: 1, index: 0, key: '1' }]) assert.throws(() => encodeRequest(27, 3, values));
  // Exact integer above 2^53, independent of any PDF-declared size or memory allocation.
  const response = Uint8Array.of(155, 3, 0, 0, 0, 6, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 32, 0, 0, 2);
  assert.deepEqual(decodeResponse(response).value, { object: { number: 6, generation: 0 }, decodedSize: 9007199254740993n, size: 0, checksum: 2 });
  for (let end = 0; end < response.length; end++) assert.throws(() => decodeResponse(response.subarray(0, end)));
  assert.throws(() => decodeResponse(Uint8Array.from([...response, 0])), /Trailing/);
  for (const position of [19, 20]) for (let value = 3; value < 256; value++) {
    const bad = response.slice(); bad[position] = value; assert.throws(() => decodeResponse(bad), /check status/);
  }
}

function dom() {
  class Element {
    children = []; listeners = new Map(); textContent = ''; disabled = false; open = false;
    set innerHTML(_) { assert.fail('Attachment results must remain inert text'); }
    addEventListener(type, callback) { this.listeners.set(type, callback); }
    fire(type) { this.listeners.get(type)?.(); }
    append(...nodes) { this.children.push(...nodes); }
    replaceChildren(...nodes) { this.children = nodes; }
    showModal() { this.open = true; }
    close() { this.open = false; this.fire('close'); }
    focus() {}
  }
  const elements = new Map();
  return { createElement: () => new Element(), getElementById: id => {
    if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id);
  } };
}

export async function testAttachmentCheckView() {
  const document = dom(), calls = [];
  let value = { decodedSize: 9007199254740993n, size: 0, checksum: 0 }, fail = false, hold = false, finish;
  const client = {
    metadata: async () => ({ info: null, language: null, metadataObject: null }),
    attachments: async () => [{ index: 7, names: { UF: '<script>../payload</script>' }, description: null,
      payloads: [{ key: 1, declaredSize: null }] }],
    attachmentCheck: async (...args) => { calls.push(args); if (hold) await new Promise(resolve => { finish = resolve; });
      if (fail) throw new Error('Attachment password required'); return value; },
  };
  const state = { client, handle: 3, epoch: 1 };
  const view = new DocumentServices({ state: () => state, document, download: () => assert.fail('Checks cannot download bytes') });
  const nodes = root => [root, ...root.children.flatMap(nodes)];
  const result = () => nodes(view.files).find(node => node.textContent === 'Size and checksum have not been checked.');
  await view.show(); assert.equal(calls.length, 0, 'listing must not check payloads');
  let output = result(); view.buttons[1].fire('click'); await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(calls, [[3, 7, 1]], 'the selected catalog index and EF key are exact');
  assert.match(output.textContent, /9007199254740993 decoded bytes/);
  assert.match(output.textContent, /Declared size: not declared\. Declared checksum: not declared/);
  for (const [size, checksum, message] of [[1, 1, /check completed/], [2, 1, /do not match/], [1, 2, /do not match/], [2, 2, /do not match/]]) {
    value = { decodedSize: 18n, size, checksum }; await view.check({ ...state }, view.serial, 7, 1, output);
    assert.match(view.message.textContent, message); assert.match(output.textContent, /18 decoded bytes/);
  }
  fail = true; await view.check({ ...state }, view.serial, 7, 1, output);
  assert.match(output.textContent, /Check unavailable:.*password required/); assert.equal(view.buttons.every(button => !button.disabled), true);
  fail = false; hold = true;
  for (const stale of [() => state.epoch++, () => state.handle++, () => { state.client = { ...client }; }, () => view.dialog.close()]) {
    await view.show(); output = result();
    const snapshot = { ...state }, checking = view.check(snapshot, view.serial, 7, 1, output), count = calls.length;
    view.buttons[1].fire('click'); assert.equal(calls.length, count, 'duplicate checks are bounded');
    stale(); finish(); await checking;
    assert.equal(output.textContent, 'Checking size and checksum…', 'stale results cannot update the old dialog');
    state.client = client;
  }
  // A pending callback retains values from show(), even if state() returns a mutable object.
  hold = false; await view.show(); const before = calls.length; state.handle++;
  view.buttons[1].fire('click'); await Promise.resolve(); assert.equal(calls.length, before, 'stale button cannot target another PDF');
  view.reset();
  console.log('Attachment-check viewer: explicit checks, exact sizes, missing declarations, failures, duplicate clicks and stale-state guards passed.');
}
