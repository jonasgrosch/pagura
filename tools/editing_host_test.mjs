/** Hosted test of the browser's actual client, WASI bridge and form sample. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';
import { SessionClient } from '../web/client.mjs';
import { runHost } from '../web/worker.mjs';
import { formDocument } from '../web/form_document.mjs';
import { testFormEditor } from './form_editor_test.mjs';
import { encodeChanges } from '../web/editing_protocol.mjs';
if (!isMainThread) {
  try { await runHost(workerData.module, workerData.shared, (message, transfer = []) => parentPort.postMessage(message, transfer)); }
  catch (error) { parentPort.postMessage({ type: 'fatal', message: String(error) }); }
} else {
  await testFormEditor();
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
    const tile = { x: 0, y: 0, width: 600, height: 440 };
    const beforePixels = await client.renderTile(opened.document, 0, 1, tile);
    const fields = await client.formFields(opened.document);
    assert.deepEqual(fields.map((f) => f.name), ['Full name', 'Team', 'Subscribe', 'Preferred contact']);
    const widgets = await client.formWidgets(opened.document, 0);
    assert.deepEqual(widgets.map((w) => w.field), [0, 1, 2, 3, 3]);
    assert.deepEqual(widgets[0].rect, [40, 300, 380, 328]);
    assert.equal(widgets[3].flags & 1, 1);
    for (const options of [{ hiddenText: true }, { omitText: true }]) {
      const hidden = await client.open(formDocument(options), 0);
      assert.deepEqual((await client.formWidgets(hidden.document, 0)).map((w) => w.field), [1, 2, 3, 3]);
      await client.close(hidden.document);
    }
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
    const afterPixels = await client.renderTile(updated.document, 0, 1, tile);
    assert.notDeepEqual(afterPixels.rgba, beforePixels.rgba, 'saved form changes the page preview');
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
    const duplicated = formDocument({ duplicateText: true }), duplicateView = await client.open(duplicated, 0);
    assert.deepEqual(await client.saveForm(duplicateView.document, []), duplicated, 'empty saves preserve original bytes even with duplicate widgets');
    assert.equal((await client.formWidgets(duplicateView.document, 0)).length, 5, 'exact duplicated widget remains interactive');
    const duplicateSaved = await client.saveForm(duplicateView.document, [{ index: 0, kind: 0, value: 'Reconciled' }]);
    assert.deepEqual(duplicateSaved.subarray(0, duplicated.length), duplicated);
    const duplicateReopened = await client.open(duplicateSaved, 0);
    const duplicatePixels = await client.renderTile(duplicateReopened.document, 0, 1, tile);
    const canonicalSaved = await client.saveForm(opened.document, [{ index: 0, kind: 0, value: 'Reconciled' }]);
    const canonicalReopened = await client.open(canonicalSaved, 0);
    assert.deepEqual(duplicatePixels.rgba, (await client.renderTile(canonicalReopened.document, 0, 1, tile)).rgba, 'reconciled references display saved field appearance');
    const different = await client.open(formDocument({ duplicateText: true, differentText: true }), 0);
    assert.equal((await client.formWidgets(different.document, 0)).length, 4, 'same name and rectangle do not establish widget identity');
    const ambiguous = await client.open(formDocument({ duplicateText: true, ambiguousText: true }), 0);
    await assert.rejects(client.formWidgets(ambiguous.document, 0), /ambiguous/);
    // Independently authored analogue of a real-world colour-operator mismatch.
    // Display recovery must not force saving through a repaired document.
    const compatibleBytes = formDocument({ colourRepair: true });
    const compatible = await client.open(compatibleBytes, 1), strict = await client.open(compatibleBytes, 0);
    await client.renderTile(compatible.document, 0, 1, tile);
    await assert.rejects(client.renderTile(strict.document, 0, 1, tile), /SCN or scn/);
    assert.ok((await client.diagnostics(compatible.document)).some((d) => d.tag === 'content.colour-operator'));
    const filledCompatible = await client.saveForm(strict.document, [{ index: 0, kind: 0, value: 'Compatible view' }]);
    assert.deepEqual(filledCompatible.subarray(0, compatibleBytes.length), compatibleBytes);
    const compatibleAgain = await client.open(filledCompatible, 1);
    await client.renderTile(compatibleAgain.document, 0, 1, tile);
    assert.equal((await client.formFields(compatibleAgain.document))[0].value, 'Compatible view');
    const icc = new Uint8Array(await readFile('corpus/generated/content/repair-icc-colour.pdf'));
    const iccView = await client.open(icc, 1);
    await client.renderTile(iccView.document, 0, 1, { x: 0, y: 0, width: 8, height: 8 });
    assert.ok((await client.diagnostics(iccView.document)).some((d) => d.tag === 'content.colour-operator'));
    console.log('Browser editing bridge: all four field kinds, strict reopen, original prefix, repeated saves, failed-save atomicity and malformed changes passed.');
  } finally { client.destroy(); }
}
