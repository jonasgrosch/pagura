/** Source-owned explicit Repair contracts; no CLI scan eligibility inference. */
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';

const root = fileURLToPath(new URL('../', import.meta.url));
const self = fileURLToPath(import.meta.url);
const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
const fileSha = (name) => sha(fs.readFileSync(name));
const sourceFile = (name) => {
  const resolved = path.resolve(root, name);
  assert(!path.relative(root, resolved).startsWith('..') && resolved !== root, 'source path escapes root');
  return resolved;
};
const durable = (name, bytes, flag = 'w') => {
  const fd = fs.openSync(name, flag);
  try { fs.writeFileSync(fd, bytes); fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
};
const json = (name, value) => durable(name, `${JSON.stringify(value, null, 2)}\n`);

if (process.argv[2] === '--wasi-child') {
  const { WASI } = await import('node:wasi');
  const [modulePath, output, requestJson] = process.argv.slice(3);
  const request = JSON.parse(requestJson);
  const wasi = new WASI({ version: 'preview1', args: ['font_directory_probe', request.operation,
    `/repo/${request.input}`, `/out/${request.ordinal}`, `${request.dpi}`, request.mode,
    request.format, request.budget], env: {}, preopens: { '/repo': root, '/out': output }, returnOnExit: true });
  const module = await WebAssembly.compile(fs.readFileSync(modulePath));
  const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
  process.exitCode = Number(wasi.start(instance) ?? 0);
} else {
  main();
}

function normalizedError(stderr, variant) {
  const lines = stderr.split('\n');
  let removed = 0;
  for (let i = 0; i < lines.length; i++) {
    if (/^\(node:\d+\) ExperimentalWarning: WASI is an experimental feature and might change at any time$/.test(lines[i])) {
      assert.notEqual(variant, 'native');
      assert.equal(removed, 0);
      assert.equal(lines[i + 1], '(Use `node --trace-warnings ...` to show where the warning was created)');
      assert(i + 2 < lines.length, 'known warning must retain its complete newline');
      lines.splice(i, 2); removed++; i--;
    }
  }
  return lines.join('\n');
}
function errorLine(detail, operation) {
  const object = detail.object ? `ObjectId { number: ${detail.object[0]}, generation: ${detail.object[1]} }` : null;
  const position = operation === 'render' ? `Stream { object: ${object}, offset: ${detail.position} }`
    : detail.position === null ? 'Unknown' : `Buffer(${detail.position})`;
  const repair = detail.repair ? `Some(Repair(${JSON.stringify(detail.repair)}))` : 'None';
  return `Error: Error { kind: ${detail.kind}, message: ${JSON.stringify(detail.message)}, position: ${position}, object: ${object ? `Some(${object})` : 'None'}, repair: ${repair} }`;
}
function capacity() {
  const python = process.env.PAGURA_PYTHON ?? (process.platform === 'win32' ? 'python' : 'python3');
  const p = spawnSync(python, ['-c', 'import shutil; print(shutil.disk_usage(".").free)'], { cwd: root, encoding: 'utf8', timeout: 10000 });
  assert.ifError(p.error); assert.equal(p.status, 0, p.stderr);
  const free = Number(p.stdout.trim()); assert(Number.isSafeInteger(free));
  assert(free >= 3 * 1024 ** 3, 'three GiB runtime headroom is required');
  return free;
}
function main() {
  const [moduleFile = 'target/font-directory-frozen/modules.json', destination = 'target/font-directory-evidence'] = process.argv.slice(2);
  const output = path.resolve(root, destination);
  assert(path.relative(path.join(root, 'target'), output) && !path.relative(path.join(root, 'target'), output).startsWith('..'));
  assert(!fs.existsSync(output), 'evidence destination must be fresh');
  assert(!fs.lstatSync(path.join(root, 'target')).isSymbolicLink(), 'target must be owned, not a symlink');
  fs.mkdirSync(output);
  const journal = path.join(output, 'attempts.jsonl');
  const event = (value) => durable(journal, `${JSON.stringify(value)}\n`, 'a');
  const pins = {};
  const pin = (name) => { pins[name] = fileSha(name); return pins[name]; };
  const modulesPath = sourceFile(moduleFile); pin(modulesPath);
  const built = JSON.parse(fs.readFileSync(modulesPath, 'utf8'));
  const git = (...args) => {
    const p = spawnSync('git', args, { cwd: root, encoding: 'utf8', timeout: 10000 });
    assert.ifError(p.error); assert.equal(p.status, 0, p.stderr); return p.stdout.trim();
  };
  assert.equal(git('rev-parse', 'HEAD'), built.source_revision);
  assert.equal(git('rev-parse', 'HEAD^{tree}'), built.source_tree);
  assert.equal(git('status', '--porcelain'), '');
  assert(built.compiler.startsWith('rustc 1.99.1 '));
  assert.equal(built.profile, 'unchanged source release'); assert.equal(built.features, 'pagura defaults');
  assert.deepEqual(Object.keys(built.modules), ['native', 'simd', 'scalar']);
  const flags = { native: null, simd: '-C\x1ftarget-feature=+simd128\x1f-C\x1foverflow-checks=on', scalar: '-C\x1ftarget-feature=-simd128\x1f-C\x1foverflow-checks=on' };
  for (const [name, digest] of Object.entries(built.sources)) assert.equal(pin(sourceFile(name)), digest);
  for (const [variant, module] of Object.entries(built.modules)) {
    assert.equal(module.source_revision, built.source_revision);
    assert.equal(module.rustflags, flags[variant]); assert.equal(pin(sourceFile(module.path)), module.sha256);
  }
  pin(self);
  const manifestPath = sourceFile('corpus/manifests/font_directory.json'); pin(manifestPath);
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  assert.equal(manifest.cases.length, 37);
  assert.equal(new Set(manifest.cases.map((c) => c.id)).size, 37);
  for (const c of manifest.cases) for (const f of Object.values(c.files)) assert.equal(pin(sourceFile(f.path)), f.sha256);
  const cases = manifest.cases.filter((c) => c.files['.font']);
  assert.equal(cases.length, 34); assert.equal(cases.filter((c) => c.expected_repair_render === 'rendered').length, 6);
  assert.equal(cases.filter((c) => c.expected_repair_render === 'refused').length, 28);
  const literals = new Map(manifest.cases.filter((c) => c.id.includes('/literal-')).map((c) => [c.dpi, c]));
  assert.deepEqual([...literals.keys()], [72, 73, 144]);
  const requests = [];
  for (const c of cases) {
    const contract = JSON.parse(fs.readFileSync(sourceFile(c.files['.contract.json'].path), 'utf8'));
    assert.deepEqual(c.portable_contracts, contract.portable_contracts);
    for (const mode of ['strict', 'repair']) {
      for (const operation of ['font', 'render']) {
        const expectation = contract.portable_contracts[mode][operation];
        const dpis = operation === 'render' && c.expected_repair_render === 'rendered' ? [72, 73, 144] : [72];
        for (const dpi of dpis) requests.push({ id: c.id, kind: contract.kind, operation, input: c.files[operation === 'font' ? '.font' : '.pdf'].path,
          input_sha256: c.files[operation === 'font' ? '.font' : '.pdf'].sha256, mode, dpi, format: c.format, budget: 'default', expectation });
      }
    }
  }
  for (const kind of ['ttf', 'cff-cid']) {
    const c = cases.find((c) => c.id === `font_directory/${kind}-count-plus-one`); assert(c);
    for (const [budget, message, position] of [['work', 'font work budget exceeded', 12], ['entries', 'font entry budget exceeded', 4], ['input', 'font program exceeds the byte budget', null]]) {
      requests.push({ id: `${c.id}/${budget}`, kind, operation: 'font', input: c.files['.font'].path,
        input_sha256: c.files['.font'].sha256, mode: 'repair', dpi: 72, format: c.format, budget,
        expectation: { status: 'refused', diagnostics: [], error: { kind: 'Limit', message, position, object: null, repair: null } } });
    }
  }
  assert.equal(requests.length, 166); assert.equal(requests.filter((r) => r.operation === 'render').length, 92);
  json(path.join(output, 'plan.json'), { schema: 1, source_revision: built.source_revision, compiler: built.compiler,
    node: process.version, actual_probe_calls_planned: 498, variants: flags, requests, pins });
  const rows = [], geometry = new Map();
  let attempted = 0;
  try {
    for (const [variant, module] of Object.entries(built.modules)) {
      for (const request of requests) {
        const ordinal = rows.length;
        const free = capacity();
        assert.equal(fileSha(sourceFile(request.input)), request.input_sha256);
        const outputPrefix = path.join(output, `${ordinal}`);
        const command = variant === 'native' ? [sourceFile(module.path), request.operation, sourceFile(request.input), outputPrefix,
          `${request.dpi}`, request.mode, request.format, request.budget]
          : [process.execPath, self, '--wasi-child', sourceFile(module.path), output, JSON.stringify({ ...request, ordinal })];
        event({ state: 'pending', ordinal, variant, request, command, source_revision: built.source_revision, module_sha256: module.sha256, physical_free_bytes: free });
        attempted++;
        const actual = spawnSync(command[0], command.slice(1), { cwd: root, encoding: 'utf8', timeout: 30000, maxBuffer: 16 * 1024 ** 2 });
        const row = { state: 'completed', ordinal, variant, id: request.id, operation: request.operation, dpi: request.dpi, mode: request.mode, budget: request.budget,
          child_pid: actual.pid ?? null, child_started: Number.isInteger(actual.pid) && actual.pid > 0,
          command, status: actual.status, signal: actual.signal, stdout: actual.stdout ?? '', stderr: actual.stderr ?? '',
          spawn_error: actual.error ? { name: actual.error.name, message: actual.error.message, code: actual.error.code } : null };
        // Raw completion is fsynced before output stat/hash, parsing, or assertions.
        event(row); rows.push(row); json(path.join(output, 'partial.json'), { complete: false, attempted_probe_calls: attempted, actual_probe_calls: rows.filter((r) => r.child_started).length, rows });
        const outputs = {};
        for (const suffix of ['.ppm', '.white.rgba', '.transparent.rgba']) {
          const name = outputPrefix + suffix; outputs[suffix] = fs.existsSync(name) ? { path: name, sha256: fileSha(name), bytes: fs.statSync(name).size } : null;
        }
        event({ state: 'outputs', ordinal, outputs }); row.outputs = outputs;
        assert.ifError(actual.error); assert.equal(actual.signal, null);
        const lines = row.stdout.trimEnd().split('\n');
        assert.equal(lines[0], `COMPILED_SOURCE ${built.source_revision}`);
        assert.equal(lines.filter((line) => line.startsWith('COMPILED_SOURCE ')).length, 1);
        assert(lines.every((line) => /^(COMPILED_SOURCE |DIAGNOSTIC |SOURCE_BYTES_VERIFIED$|MATRIX |GLYPHS |GLYPH |DIMENSIONS |TEXT )/.test(line)), 'unknown probe stdout');
        const tags = lines.filter((line) => line.startsWith('DIAGNOSTIC ')).map((line) => line.split(' ')[1]);
        assert.deepEqual(tags, request.expectation.diagnostics, `${request.id}/${variant}/${request.mode}: diagnostics`);
        const position = (offset) => request.operation === 'font' ? `Buffer(${offset}) None`
          : `Stream { object: ObjectId { number: 7, generation: 0 }, offset: ${offset} } Some(ObjectId { number: 7, generation: 0 })`;
        assert.deepEqual(lines.filter((line) => line.startsWith('DIAGNOSTIC ')), tags.map((tag) => {
          assert(['font.sfnt-directory-count', 'font.table-bounds'].includes(tag));
          return `DIAGNOSTIC ${tag} ${position(tag === 'font.sfnt-directory-count' ? 4 : 20)}`;
        }));
        const stderr = normalizedError(row.stderr, variant);
        if (request.expectation.status === 'refused') {
          assert.equal(row.status, 1); assert(Object.values(outputs).every((value) => value === null));
          assert.equal(stderr, `${errorLine(request.expectation.error, request.operation)}\n`);
          assert(lines.every((line) => line.startsWith('COMPILED_SOURCE ') || line.startsWith('DIAGNOSTIC ')), 'failed operation must not emit a successful or partial result');
        } else {
          assert.equal(row.status, 0); assert.equal(stderr, '');
          assert.equal(lines.filter((line) => line === 'SOURCE_BYTES_VERIFIED').length, 1);
          if (request.operation === 'render') {
            const literal = literals.get(request.dpi); assert(literal);
            assert(lines.includes(`DIMENSIONS ${literal.width} ${literal.height}`));
            assert.equal(lines.filter((line) => line.startsWith('DIMENSIONS ')).length, 1);
            assert(!lines.some((line) => line.startsWith('MATRIX ') || line.startsWith('GLYPH')));
            assert.deepEqual(lines.filter((line) => line.startsWith('TEXT ')), [
              'TEXT Some("A") Point { x: 8.0, y: 12.0 } Point { x: 68.0, y: 12.0 }',
              'TEXT Some("A") Point { x: 32.0, y: 9.0 } Point { x: 92.0, y: 9.0 }',
            ]);
            for (const suffix of ['.ppm', '.white.rgba', '.transparent.rgba']) {
              const expected = literal.files[suffix]; assert(outputs[suffix]);
              assert.equal(outputs[suffix].sha256, expected.sha256);
              assert.deepEqual(fs.readFileSync(outputs[suffix].path), fs.readFileSync(sourceFile(expected.path)));
            }
          } else {
            assert(Object.values(outputs).every((value) => value === null));
            assert(lines.includes('GLYPHS 5'));
            assert.equal(lines.filter((line) => line.startsWith('GLYPHS ')).length, 1);
            assert.equal(lines.filter((line) => line.startsWith('MATRIX ')).length, 1);
            assert(!lines.some((line) => line.startsWith('DIMENSIONS ') || line.startsWith('TEXT ')));
            assert.deepEqual(lines.filter((line) => line.startsWith('GLYPH ')).map((line) => Number(line.split(' ')[1])), [0, 1, 2, 3, 4]);
            const rectangle = lines.find((line) => line.startsWith('GLYPH 1 ')); assert(rectangle?.includes('Some(600.0)'));
            for (const segment of ['MoveTo(Point { x: 0.0, y: 0.0 })', 'LineTo(Point { x: 100.0, y: 0.0 })', 'LineTo(Point { x: 100.0, y: 200.0 })', 'LineTo(Point { x: 0.0, y: 200.0 })', 'Close']) assert(rectangle.includes(segment));
          }
          const semantic = lines.filter((line) => !line.startsWith('DIAGNOSTIC ') && !line.startsWith('COMPILED_SOURCE '));
          const key = `${request.operation}/${request.kind}/${request.dpi}`;
          if (!geometry.has(key)) { assert(request.id.endsWith('-valid')); geometry.set(key, semantic); }
          else assert.deepEqual(semantic, geometry.get(key), `${request.id}/${variant}: all source glyph/text observations`);
        }
      }
    }
    assert.equal(rows.length, 498);
    assert.equal(attempted, 498); assert(rows.every((row) => row.child_started));
    assert.equal(git('rev-parse', 'HEAD'), built.source_revision); assert.equal(git('status', '--porcelain'), '');
    for (const [name, digest] of Object.entries(pins)) assert.equal(fileSha(name), digest);
    json(path.join(output, 'receipt.json'), { complete: true, passed: true, source_revision: built.source_revision,
      attempted_probe_calls: attempted, actual_probe_calls: rows.filter((row) => row.child_started).length, native_simd_scalar: true, independent_oracle_tolerance: 0, pins, rows,
      scope: 'Explicit public font/render API only; no scan discovery, performance or memory claim' });
    console.log('PASS498 explicit font/render native/SIMD/scalar contracts');
  } catch (error) {
    json(path.join(output, 'failure.json'), { complete: true, passed: false, attempted_probe_calls: attempted, actual_probe_calls: rows.filter((row) => row.child_started).length, error: String(error), pins, rows });
    throw error;
  }
}
