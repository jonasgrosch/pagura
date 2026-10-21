import fs from 'node:fs';
import { createHash } from 'node:crypto';
import { WASI } from 'node:wasi';

const [input, requestedRoot, ...testArgs] = process.argv.slice(2);
if (!input || !requestedRoot) throw new Error('Expected module, arm root, test arguments');
const file = fs.realpathSync(input);
const root = fs.realpathSync(requestedRoot);
const bytes = fs.readFileSync(file);
const preopens = { [root]: root };
console.log(JSON.stringify({ schema: 1,
  module: { path: file, sha256: createHash('sha256').update(bytes).digest('hex') },
  test_args: testArgs, preopens,
  runtime: { executable: process.execPath, version: process.version, exec_argv: process.execArgv },
}));
const wasi = new WASI({ version: 'preview1', args: [file, ...testArgs],
  env: {}, preopens, returnOnExit: true });
const module = await WebAssembly.compile(bytes);
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
