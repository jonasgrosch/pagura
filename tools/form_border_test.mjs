/** Independent filled-border pixels through the actual WASI editing client. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

export async function testFormBorders(client) {
  const manifest = JSON.parse(await readFile('corpus/manifests/form_borders.json', 'utf8'));
  assert.equal(manifest.cases.length, 35);
  for (const entry of manifest.cases) {
    const original = new Uint8Array(await readFile(entry.files['.pdf'].path));
    const opened = await client.open(original, 0);
    const changes = entry.field_kind === 'Tx' ? [{ index: 0, kind: 0, value: entry.text }]
      : entry.field_kind === 'Ch' ? [{ index: 0, kind: 1, indices: [1] }] : [{ index: 0, kind: 2 }];
    const saved = await client.saveForm(opened.document, changes);
    assert.deepEqual(saved.subarray(0, original.length), original, entry.id);
    const reopened = await client.open(saved, 0);
    const size = 32 * entry.dpi / 72;
    const tile = await client.renderTile(reopened.document, 0, entry.dpi / 72, { x: 0, y: 0, width: size, height: size });
    const expected = await readFile(entry.files['.ppm'].path);
    const header = Buffer.from(`P6\n${size} ${size}\n255\n`);
    assert.deepEqual(expected.subarray(0, header.length), header, entry.id);
    const pixels = expected.subarray(header.length);
    assert.equal(tile.rgba.length, size * size * 4, entry.id);
    assert.equal(pixels.length, size * size * 3, entry.id);
    let worst = 0;
    for (let p = 0; p < size * size; p++) {
      assert.equal(tile.rgba[p * 4 + 3], 255, entry.id);
      for (let c = 0; c < 3; c++) worst = Math.max(worst, Math.abs(tile.rgba[p * 4 + c] - pixels[p * 3 + c]));
    }
    assert.ok(worst <= entry.tolerance, `${entry.id}: independent pixel error ${worst}`);
    await client.close(reopened.document);
    await client.close(opened.document);
  }
  console.log('Filled borders: 35 independent pixel controls passed through WASI save, strict reopen and rendering.');
}
