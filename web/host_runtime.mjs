import { createWasi, WasiExit } from './wasi.mjs';
import { inputReader } from './transport.mjs';

/** Runs the same worker/shim under browser and Node's worker_threads tests. */
export async function runHost(module, shared, send) {
  let instance, lastMemoryBytes = 0, stderr = '';
  const text = new TextDecoder();
  const input = inputReader(shared, () => send({ type: 'drain' }));
  const memory = () => {
    const wasmBytes = instance.exports.memory.buffer.byteLength;
    if (wasmBytes > 512 * 1024 * 1024) throw new RangeError('WebAssembly memory exceeds the 512 MiB host budget');
    if (wasmBytes !== lastMemoryBytes) { lastMemoryBytes = wasmBytes; send({ type: 'memory', wasmBytes }); }
  };
  const read = (bytes) => { memory(); return input(bytes); };
  const wasi = createWasi(() => instance.exports.memory, read, (fd, bytes) => {
    memory();
    if (fd === 2) stderr = (stderr + text.decode(bytes, { stream: true })).slice(-2048);
    if (!bytes.length) return;
    const copy = bytes.slice(); send({ type: fd === 1 ? 'stdout' : 'stderr', bytes: copy }, [copy.buffer]);
  });
  for (const item of WebAssembly.Module.imports(module)) {
    if (item.module !== 'wasi_snapshot_preview1' || item.kind !== 'function') throw new Error(`Unsupported module import: ${item.module}.${item.name}`);
  }
  instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi });
  send({ type: 'ready', wasmBytes: instance.exports.memory.buffer.byteLength });
  let code = 0;
  try { instance.exports._start(); }
  catch (error) {
    if (error instanceof WasiExit) code = error.code;
    else if (stderr.trim()) throw new Error(`${error}; ${stderr.trim()}`, { cause: error });
    else throw error;
  }
  send({ type: 'exit', code });
}

