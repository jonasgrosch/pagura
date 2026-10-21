import fs from 'node:fs';
import { createHash } from 'node:crypto';
import { WASI } from 'node:wasi';

const [input, ...testArgs] = process.argv.slice(2);
if (!input) throw new Error('Expected a WASI test module followed by its test arguments');
const file = fs.realpathSync(input);
const bytes = fs.readFileSync(file);
console.log(JSON.stringify({
  schema: 1,
  module: { path: file, sha256: createHash('sha256').update(bytes).digest('hex') },
  test_args: testArgs,
}));
const wasi = new WASI({
  version: 'preview1', args: [file, ...testArgs], env: {}, preopens: {}, returnOnExit: true,
});
const module = await WebAssembly.compile(bytes);
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
