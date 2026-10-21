#!/usr/bin/env node
/** One fresh Node/WASI invocation of the unchanged public-API profile example. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { open, readFile, writeFile } from 'node:fs/promises';
import { basename, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { WASI } from 'node:wasi';

assert.equal(process.argv.length, 9,
  'node run_profile_wasi.mjs MODULE INPUT MODE OUTPUT WASI_STDOUT WASI_STDERR METADATA');
assert.deepEqual(process.execArgv, [], 'Use default Node tiering and no extra Node flags');
assert.ok(!process.env.NODE_OPTIONS && !process.env.NODE_V8_COVERAGE,
  'NODE_OPTIONS and NODE_V8_COVERAGE must be unset');
const [modulePath, input, mode, output, stdoutPath, stderrPath, metadataPath] =
  process.argv.slice(2).map((value, index) => index === 2 ? value : resolve(value));
assert.ok(['exact', 'competitive'].includes(mode));
for (const path of [stdoutPath, stderrPath, metadataPath]) {
  assert.equal(dirname(path), dirname(output), 'Sample artifacts must share one private directory');
}
const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
const report = {
  schema: 1, status: 'incomplete', runtime: {
    executable: process.execPath, version: process.version, versions: process.versions,
    exec_argv: process.execArgv, platform: process.platform, arch: process.arch,
    node_options: process.env.NODE_OPTIONS ?? '',
  },
  argv: process.argv, wasi_args: ['scene_index_profile', `/input/${basename(input)}`,
    mode, 'full', `/output/${basename(output)}`, '72'],
  wasi_env: {}, preopens: { '/input': dirname(input), '/output': dirname(output) },
  linear_memory: { before_start_bytes: null, after_start_bytes: null,
    scope: 'Exported WASM linear memory; not Node process RSS or render-only allocation' },
};
let instance;
const handles = [];
const take = async (path, flags) => {
  const handle = await open(path, flags); handles.push(handle); return handle;
};
try {
  const bytes = await readFile(modulePath);
  report.module_sha256 = sha(bytes);
  report.input_sha256 = sha(await readFile(input));
  report.wrapper_sha256 = sha(await readFile(fileURLToPath(import.meta.url)));
  // Compile/instantiate are outside the Rust example's internal timers. Lazy
  // function compilation and tiering during its first render may remain inside.
  const module = await WebAssembly.compile(bytes);
  const stdin = await take(resolve(dirname(output), 'empty.stdin'), 'wx+');
  const stdout = await take(stdoutPath, 'wx');
  const stderr = await take(stderrPath, 'wx');
  const wasi = new WASI({ version: 'preview1', args: report.wasi_args,
    env: {}, preopens: report.preopens, returnOnExit: true,
    stdin: stdin.fd, stdout: stdout.fd, stderr: stderr.fd });
  instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
  if (instance.exports.memory instanceof WebAssembly.Memory) {
    report.linear_memory.before_start_bytes = instance.exports.memory.buffer.byteLength;
  }
  report.exit_code = wasi.start(instance);
  assert.equal(report.exit_code, 0, 'WASI program failed');
  report.status = 'completed';
} catch (error) {
  report.status = 'failed';
  report.error = { name: error.name, message: error.message, stack: error.stack };
  process.exitCode = 1;
} finally {
  if (instance?.exports.memory instanceof WebAssembly.Memory) {
    report.linear_memory.after_start_bytes = instance.exports.memory.buffer.byteLength;
  }
  for (const handle of handles.reverse()) await handle.close();
  await writeFile(metadataPath, JSON.stringify(report, null, 2) + '\n', { flag: 'wx' });
}
