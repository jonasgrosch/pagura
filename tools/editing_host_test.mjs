/** Hosted test of the browser's actual client, WASI bridge and form sample. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';
import { SessionClient } from '../web/client.mjs';
import { runHost } from '../web/worker.mjs';
import { formDocument } from '../web/form_document.mjs';
import { encodeChanges } from '../web/editing_protocol.mjs';
if (!isMainThread) {
  try { await runHost(workerData.module, workerData.shared, (message, transfer = []) => parentPort.postMessage(message, transfer)); }
  catch (error) { parentPort.postMessage({ type: 'fatal', message: String(error) }); }
} else {
  const module = await WebAssembly.compile(await readFile(process.argv[2] ?? 'target/wasm32-wasip1/release/pagura-host.wasm'));
  class BrowserWorker {
    listeners = new Map();
    addEventListener(type, callback) { const list = this.listeners.get(type) ?? []; list.push(callback); this.listeners.set(type, list); }
    postMessage(message) {
      assert.equal(message.type, 'start');
      this.worker = new Worker(new URL(import.meta.url), { workerData: { module, shared: message.shared } });
      this.worker.on('message', (data) => { for (const callback of this.listeners.get('message') ?? []) callback({ data }); });
      this.worker.on('error', (error) => { for (const callback of this.listeners.get('error') ?? []) callback(error); });
    }
    terminate() { void this.worker?.terminate(); }
  }
  const client = new SessionClient(new BrowserWorker());
  try {
    await client.hello();
    const original = formDocument(), opened = await client.open(original, 0);
    const fields = await client.formFields(opened.document);
    assert.deepEqual(fields.map((f) => f.name), ['Full name', 'Team', 'Subscribe', 'Preferred contact']);
    const yes = fields[2].options[0], phone = fields[3].options.find((o) => o.label === 'Phone');
    const changes = [
      { index: 0, kind: 0, value: 'Jonas Browser Test' },
      { index: 1, kind: 1, indices: [2] },
      { index: 2, kind: 3, widget: yes.key, state: yes.state },
      { index: 3, kind: 3, widget: phone.key, state: phone.state },
    ];
    const saved = await client.saveForm(opened.document, changes);
    assert.deepEqual(saved.subarray(0, original.length), original);
    assert.deepEqual(await client.formFields(opened.document), fields, 'source remains immutable');
    const updated = await client.open(saved, 0), after = await client.formFields(updated.document);
    assert.equal(after[0].value, 'Jonas Browser Test');
    assert.equal(after[1].options.find((o) => o.selected).label, 'Research');
    assert.equal(after[2].options.filter((o) => o.selected).length, 1);
    assert.equal(after[3].options.find((o) => o.selected).label, 'Phone');
    await assert.rejects(client.saveForm(updated.document, [{ index: 0, kind: 0, value: 'x'.repeat(81) }]));
    assert.deepEqual(await client.formFields(updated.document), after, 'failed save leaves snapshot usable');
    const savedAgain = await client.saveForm(updated.document, [{ index: 0, kind: 0, value: 'Second edit' }]);
    assert.deepEqual(savedAgain.subarray(0, saved.length), saved);
    const reopened = await client.open(savedAgain, 0);
    assert.equal((await client.formFields(reopened.document))[0].value, 'Second edit');
    assert.throws(() => encodeChanges([changes[0], changes[0]]), /duplicate/);
    assert.throws(() => encodeChanges([{ index: 0, kind: 1, indices: [-1] }]), /Invalid/);
    console.log('Browser editing bridge: all four field kinds, strict reopen, original prefix, repeated saves, failed-save atomicity and malformed changes passed.');
  } finally { client.destroy(); }
}
