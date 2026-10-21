#!/usr/bin/env node
/** Two independent opaque-composition checks through native and Node WASI hosts.
 * Copy to benchmarks/opaque_host_probe.mjs; run only after serial builds finish:
 * node benchmarks/opaque_host_probe.mjs --outputdir <new-directory> --run-id <id>
 * Optional --root selects the checkout. No browser shim or performance claim.
 * The protocol exposes default Exact only; Competitive is covered by native Rust tests.
 */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, open, readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';
import { WASI } from 'node:wasi';

const options = {};
for (let i = 2; i < process.argv.length; i += 2) {
  const name = process.argv[i], value = process.argv[i + 1];
  if (!['--root', '--outputdir', '--output-dir', '--run-id'].includes(name) || !value || value.startsWith('--')) {
    throw new Error('Usage: opaque_host_probe.mjs [--root checkout] --outputdir NEW_DIRECTORY --run-id ID');
  }
  const key = name === '--output-dir' ? '--outputdir' : name;
  assert.equal(options[key], undefined, `Duplicate ${key}`);
  options[key] = value;
}
assert.ok(options['--outputdir'] && options['--run-id'], '--outputdir and --run-id are required');
const script = fileURLToPath(import.meta.url);
const root = resolve(options['--root'] ?? join(dirname(script), '..'));
const output = resolve(options['--outputdir']);
// Refuse an existing output directory so retained evidence is never overwritten.
await mkdir(output);
const native = join(root, 'target/debug/pagura-host');
const wasmPath = join(root, 'target/wasm32-wasip1/release/pagura-host.wasm');
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const report = {
  schema: 1, run_id: options['--run-id'], status: 'incomplete', started_at: new Date().toISOString(),
  root, node: process.version, platform: process.platform, arch: process.arch,
  scope: 'Two direct-root precision fixtures; default Exact native/Node-WASI host parity. No performance claim.',
  reference_model: 'Normal source-over at the independently specified coverage bytes, not continuous geometry.',
  competitive: 'Not exercised by this host protocol; covered by the separate native Rust regression tests.',
  build_binding: 'Observed source and binary hashes; source-to-binary provenance requires the coordinator build records.',
  artifacts: {}, cases: [],
};
function git(args) {
  const result = spawnSync('git', ['-C', root, ...args], { encoding: 'utf8', timeout: 30000, maxBuffer: 16 * 1024 * 1024 });
  assert.ifError(result.error); assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}
async function sourceIdentity() {
  const paths = [...new Set(git(['ls-files', '--cached', '--others', '--exclude-standard', '-z', '--',
    'crates', 'Cargo.toml', 'Cargo.lock', '.cargo', 'rust-toolchain.toml', 'web', 'tools/fixturegen/content.py'])
    .split('\0').filter((p) => /\.(rs|toml|lock|mjs|py)$/.test(p)))].sort();
  paths.push('crates/pagura-host/PROTOCOL.md', 'corpus/manifests/content.json');
  const files = {};
  for (const path of paths) files[path] = sha256(await readFile(join(root, path)));
  const identity = { head: git(['rev-parse', 'HEAD']).trim(), files };
  return { ...identity, sha256: sha256(Buffer.from(JSON.stringify(identity))) };
}
async function retain(name, bytes) {
  await writeFile(join(output, name), bytes, { flag: 'wx' });
  report.artifacts[name] = { bytes: bytes.length, sha256: sha256(bytes) };
}
function frames(bytes, FrameDecoder) {
  const decoded = [], decoder = new FrameDecoder((frame) => decoded.push(frame));
  for (let i = 0; i < bytes.length; i += 137) decoder.feed(bytes.subarray(i, i + 137));
  decoder.finish(); return decoded;
}
async function wasiRun(module, prefix, inputPath) {
  const handles = [];
  const take = async (path, flags) => { const handle = await open(path, flags); handles.push(handle); return handle; };
  let code, failure;
  try {
    const stdin = await take(inputPath, 'r');
    const stdout = await take(join(output, `${prefix}.wasi.response.bin`), 'wx');
    const stderr = await take(join(output, `${prefix}.wasi.stderr`), 'wx');
    const wasi = new WASI({ version: 'preview1', args: [], env: {}, preopens: {}, returnOnExit: true,
      stdin: stdin.fd, stdout: stdout.fd, stderr: stderr.fd });
    const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
    code = wasi.start(instance);
  } catch (error) { failure = error; }
  finally { for (const handle of handles.reverse()) await handle.close(); }
  for (const suffix of ['response.bin', 'stderr']) {
    const name = `${prefix}.wasi.${suffix}`, bytes = await readFile(join(output, name));
    report.artifacts[name] = { bytes: bytes.length, sha256: sha256(bytes) };
  }
  if (failure) throw failure;
  return { code, bytes: await readFile(join(output, `${prefix}.wasi.response.bin`)) };
}
try {
  report.source_before = await sourceIdentity();
  report.probe_sha256 = sha256(await readFile(script));
  const nativeBytes = await readFile(native), wasmBytes = await readFile(wasmPath);
  report.binaries = { native: { path: native, sha256: sha256(nativeBytes) },
    wasi: { path: wasmPath, sha256: sha256(wasmBytes) } };
  report.protocol = {};
  for (const path of ['web/protocol.mjs', 'web/reading_protocol.mjs', 'crates/pagura-host/PROTOCOL.md']) {
    report.protocol[path] = sha256(await readFile(join(root, path)));
  }
  const { FrameDecoder, encodeRequest, VERSION_1_4 } = await import(pathToFileURL(join(root, 'web/protocol.mjs')).href);
  const module = await WebAssembly.compile(wasmBytes);
  const manifest = JSON.parse(await readFile(join(root, 'corpus/manifests/content.json'), 'utf8'));
  const cases = [
    { stem: 'precision-root-two-partial', lo: 218, hi: 219, numerator: 44617n, denominator: 204n,
      formula: '44617/204' },
    { stem: 'precision-root-slivers-1000', lo: 5, hi: 6, numerator: 254n ** 1000n, denominator: 255n ** 999n,
      formula: '255*(254/255)^1000' },
  ];
  for (const test of cases) {
    const matches = manifest.cases.filter((entry) => entry.id === `content/${test.stem}`);
    assert.equal(matches.length, 1, 'Unique manifest fixture');
    const entry = matches[0], path = `corpus/generated/content/${test.stem}.pdf`;
    assert.equal(entry.path, path); assert.equal(entry.expected, 'render'); assert.equal(entry.strict_valid, true);
    assert.deepEqual(entry.repairs, []); assert.equal(entry.width, 1); assert.equal(entry.height, 1); assert.equal(entry.dpi, 72);
    const pdf = await readFile(join(root, path));
    assert.equal(sha256(pdf), entry.sha256, 'Manifest input identity');
    await retain(`${test.stem}.pdf`, pdf);
    const input = Buffer.concat([
      encodeRequest(1, 1, { version: VERSION_1_4 }),
      encodeRequest(2, 2, { bytes: pdf, recovery: 0, password: new Uint8Array() }),
      encodeRequest(3, 3, { document: 1, page: 0 }),
      encodeRequest(4, 4, { document: 1, page: 0, scale: 1, x: 0, y: 0, width: 1, height: 1 }),
      encodeRequest(5, 5, { document: 1 }), encodeRequest(17, 6, { document: 1 }),
      encodeRequest(6, 7, { document: 1 }),
    ]);
    await retain(`${test.stem}.request.bin`, input);
    const result = { id: entry.id, status: 'incomplete', input_sha256: sha256(pdf), request_sha256: sha256(input),
      oracle: { formula: test.formula, valid_rgb_inclusive: [test.lo, test.hi], alpha: 255, maximum_absolute_error: 1 } };
    report.cases.push(result);
    const ran = spawnSync(native, [], { input, timeout: 30000, maxBuffer: 1024 * 1024 });
    await retain(`${test.stem}.native.response.bin`, ran.stdout ?? Buffer.alloc(0));
    await retain(`${test.stem}.native.stderr`, ran.stderr ?? Buffer.alloc(0));
    result.native = { command: [native], status: ran.status, signal: ran.signal, error: ran.error?.message ?? null };
    assert.ifError(ran.error); assert.equal(ran.status, 0, ran.stderr?.toString());
    const wasi = await wasiRun(module, test.stem, join(output, `${test.stem}.request.bin`));
    result.wasi = { exit_code: wasi.code, args: [], env: {}, preopens: {} };
    assert.equal(wasi.code, 0); assert.deepEqual(wasi.bytes, ran.stdout, 'Exact raw native/WASI response parity');
    const decoded = frames(wasi.bytes, FrameDecoder);
    assert.deepEqual(decoded.map((frame) => frame.kind), [129, 130, 131, 132, 133, 145, 134]);
    assert.deepEqual(decoded.map((frame) => frame.id), [1, 2, 3, 4, 5, 6, 7]);
    assert.deepEqual(decoded[0].value, { version: VERSION_1_4 });
    assert.deepEqual(decoded[1].value, { document: 1, pages: 1 });
    assert.deepEqual(decoded[2].value, { crop: [0, 0, 1, 1], rotation: 0, userUnit: 1, width: 1, height: 1 });
    assert.deepEqual(decoded[4].value, []); assert.deepEqual(decoded[5].value, []);
    const tile = decoded[3].value;
    assert.equal(tile.width, 1); assert.equal(tile.height, 1); assert.equal(tile.rgba.length, 4);
    assert.equal(tile.rgba[3], 255);
    for (const byte of tile.rgba.subarray(0, 3)) {
      assert.ok(byte >= test.lo && byte <= test.hi, `Strict rational RGB interval: got ${byte}`);
      const error = BigInt(byte) * test.denominator - test.numerator;
      assert.ok((error < 0n ? -error : error) <= test.denominator, 'Absolute error against unrounded rational is at most one');
    }
    Object.assign(result, { status: 'passed', rgba: [...tile.rgba], raw_response_parity: true, diagnostics: [] });
  }
  report.source_after = await sourceIdentity();
  assert.deepEqual(report.source_after, report.source_before, 'Source changed during probe');
  assert.equal(sha256(await readFile(script)), report.probe_sha256, 'Probe changed during execution');
  for (const binary of Object.values(report.binaries)) assert.equal(sha256(await readFile(binary.path)), binary.sha256, 'Binary changed during probe');
  report.status = 'passed';
} catch (error) {
  report.status = 'failed'; report.error = { name: error.name, message: error.message, stack: error.stack };
  process.exitCode = 1;
} finally {
  report.completed_at = new Date().toISOString();
  const bytes = Buffer.from(JSON.stringify(report, null, 2) + '\n');
  await writeFile(join(output, 'report.json'), bytes, { flag: 'wx' });
  await writeFile(join(output, 'report.json.sha256'), sha256(bytes) + '\n', { flag: 'wx' });
  console.log(`${report.status}: ${join(output, 'report.json')}`);
}
