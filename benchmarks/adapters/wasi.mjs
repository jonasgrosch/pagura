// Node/V8 WASI preview1. Compilation/startup is outside Rust conversion timers.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { WASI } from 'node:wasi';

const [modulePath, source, target, ...args] = process.argv.slice(2);
if (!modulePath || !source || !target) throw new Error('expected WASM INPUT OUTDIR DPI FIRST LAST');
const wasi = new WASI({ version: 'preview1', returnOnExit: true,
  args: ['pagura-wasm-convert', `/input/${path.basename(source)}`, '/output', ...args],
  env: {}, preopens: { '/input': path.dirname(path.resolve(source)), '/output': path.resolve(target) } });
const module = await WebAssembly.compile(readFileSync(modulePath));
const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
process.exitCode = wasi.start(instance);
