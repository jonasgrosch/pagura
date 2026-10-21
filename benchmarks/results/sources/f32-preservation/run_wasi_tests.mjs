import fs from 'node:fs';
import crypto from 'node:crypto';
import path from 'node:path';
import { WASI } from 'node:wasi';

// Fixture helpers embed the build-time absolute workspace path. Only that
// workspace is preopened; the test runner performs no rendering benchmarks.
const [file, workspace, filter] = process.argv.slice(2);
if (!file || !workspace || !filter || process.argv.length !== 5) {
  throw new Error('Expected module, workspace, and test filter');
}
const root = fs.realpathSync(workspace);
if (!path.isAbsolute(root)) throw new Error('Expected absolute workspace');
const bytes = fs.readFileSync(file);
console.log(JSON.stringify({ module: fs.realpathSync(file),
  sha256: crypto.createHash('sha256').update(bytes).digest('hex'),
  filter, preopens: [root] }));
const wasi = new WASI({ version: 'preview1',
  args: ['f32-preservation-test', filter, '--nocapture', '--test-threads=1'],
  env: {}, preopens: { [root]: root }, returnOnExit: true });
const module = await WebAssembly.compile(bytes);
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
