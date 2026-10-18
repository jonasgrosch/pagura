/** Exploratory tile throughput; no cross-engine or release performance claim.
 * node web/tile_benchmark.ts NATIVE_BINARY WASM_BINARY > /tmp/tiles.json
 * Two warmups, five samples, identical 64 tiles and independent PPM verification.
 * Native samples include process startup; WASI samples include _start/open but
 * exclude module compilation/instantiation. Compare revisions within a runtime.
 */
import { readFileSync, writeFileSync, mkdtempSync, openSync, closeSync, rmSync } from 'node:fs';
import { tmpdir, cpus, release } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { WASI } from 'node:wasi';
import assert from 'node:assert/strict';
import { encodeRequest, FrameDecoder } from './protocol.mjs';
import type { Frame, Tile, TileRect } from './protocol.mjs';

const [nativePath, wasmPath] = process.argv.slice(2);
if (!nativePath || !wasmPath) throw new Error('Expected native binary and WASM binary paths');
const root = new URL('../', import.meta.url),
  pdf = readFileSync(new URL('corpus/generated/classic/many-rectangles.pdf', root));
const ppm = readFileSync(new URL('corpus/generated/classic/many-rectangles.ppm', root));
const rgb = ppm.subarray(Buffer.byteLength('P6\n512 512\n255\n'));
const tiles: TileRect[] = [];
for (let y = 0; y < 512; y += 64) for (let x = 0; x < 512; x += 64) tiles.push({ x, y, width: 64, height: 64 });
const input = Buffer.concat([
  encodeRequest(1, 1),
  encodeRequest(2, 2, { bytes: pdf, password: new Uint8Array(), recovery: 0 }),
  ...tiles.map((tile, index) => encodeRequest(4, index + 3, { document: 1, page: 0, scale: 1, ...tile })),
]);
const module = await WebAssembly.compile(readFileSync(wasmPath));
const scratch = mkdtempSync(join(tmpdir(), 'pagura-tiles-'));
const digest = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');
function verify(bytes: Uint8Array) {
  const frames: Frame[] = [],
    decoder = new FrameDecoder((frame) => frames.push(frame));
  decoder.feed(bytes);
  decoder.finish();
  assert.equal(frames.length, tiles.length + 2);
  for (const [index, tile] of tiles.entries()) {
    const frame = frames[index + 2];
    assert.equal(frame.kind, 132);
    const rgba = (frame.value as Tile).rgba;
    for (let y = 0; y < 64; y++)
      for (let x = 0; x < 64; x++) {
        const at = (y * 64 + x) * 4,
          expected = ((tile.y + y) * 512 + tile.x + x) * 3;
        for (let c = 0; c < 3; c++) assert.equal(rgba[at + c], rgb[expected + c]);
        assert.equal(rgba[at + 3], 255);
      }
  }
  return digest(bytes);
}
const samples = [];
try {
  for (let sample = -2; sample < 5; sample++) {
    for (const runtime of sample % 2 ? ['wasi', 'native'] : ['native', 'wasi']) {
      let seconds: number, output: Buffer;
      if (runtime === 'native') {
        const start = performance.now();
        const result = spawnSync(nativePath, [], { input, maxBuffer: 32 * 1024 * 1024, timeout: 60000 });
        seconds = (performance.now() - start) / 1000;
        assert.ifError(result.error);
        assert.equal(result.status, 0, result.stderr?.toString());
        output = result.stdout;
      } else {
        writeFileSync(join(scratch, 'input'), input);
        const fds = [
          openSync(join(scratch, 'input'), 'r'),
          openSync(join(scratch, 'output'), 'w+'),
          openSync(join(scratch, 'error'), 'w+'),
        ];
        try {
          const wasi = new WASI({
            version: 'preview1',
            args: [],
            env: {},
            preopens: {},
            returnOnExit: true,
            stdin: fds[0],
            stdout: fds[1],
            stderr: fds[2],
          });
          const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
          const start = performance.now();
          const status = wasi.start(instance);
          seconds = (performance.now() - start) / 1000;
          assert.equal(status, 0);
          output = readFileSync(join(scratch, 'output'));
        } finally {
          for (const fd of fds) closeSync(fd);
        }
      }
      const outputSha256 = verify(output);
      if (sample >= 0) samples.push({ runtime, sample, seconds, tilesPerSecond: tiles.length / seconds, outputSha256 });
    }
  }
  process.stdout.write(
    `${JSON.stringify(
      {
        schema: 1,
        workload: 'many-rectangles',
        tiles: tiles.length,
        tileSize: 64,
        scale: 1,
        pdfSha256: digest(pdf),
        nativeSha256: digest(readFileSync(nativePath)),
        wasmSha256: digest(readFileSync(wasmPath)),
        node: process.version,
        platform: process.platform,
        os: release(),
        cpu: cpus()[0]?.model,
        samples,
      },
      null,
      2,
    )}\n`,
  );
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
