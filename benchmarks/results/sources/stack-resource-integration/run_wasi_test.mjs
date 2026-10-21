import fs from 'node:fs';
import { WASI } from 'node:wasi';
const [file, filter] = process.argv.slice(2);
const wasi = new WASI({ version: 'preview1', args: ['glyph-budget-test', filter, '--exact', '--nocapture', '--test-threads=1'], env: {}, preopens: {}, returnOnExit: true });
const module = await WebAssembly.compile(fs.readFileSync(file));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
