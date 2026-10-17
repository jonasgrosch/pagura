import { createWasi, WasiExit } from './wasi.mjs';
import { inputReader } from './transport.mjs';

/** Runs the same worker/shim under browser and Node's worker_threads tests. */
export async function runHost(module, shared, send) {
  let instance;
  const read = inputReader(shared, () => send({ type: 'drain' }));
  const wasi = createWasi(() => instance.exports.memory, read, (fd, bytes) => {
    if (!bytes.length) return;
    const copy = bytes.slice(); send({ type: fd === 1 ? 'stdout' : 'stderr', bytes: copy }, [copy.buffer]);
  });
  for (const item of WebAssembly.Module.imports(module)) {
    if (item.module !== 'wasi_snapshot_preview1' || item.kind !== 'function') throw new Error(`Unsupported module import: ${item.module}.${item.name}`);
  }
  instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi });
  send({ type: 'ready' });
  let code = 0;
  try { instance.exports._start(); }
  catch (error) { if (!(error instanceof WasiExit)) throw error; code = error.code; }
  send({ type: 'exit', code });
}

if (typeof globalThis.addEventListener === 'function') {
  globalThis.addEventListener('message', async ({ data }) => {
    if (data.type !== 'start') return;
    try {
      const response = await fetch(new URL('./pagura-host.wasm', import.meta.url));
      if (!response.ok) throw new Error(`Cannot load pagura-host.wasm: HTTP ${response.status}. Run cargo wasm first.`);
      const module = await WebAssembly.compile(await response.arrayBuffer());
      await runHost(module, data.shared, (message, transfer = []) => globalThis.postMessage(message, transfer));
    } catch (error) { globalThis.postMessage({ type: 'fatal', message: String(error) }); }
  }, { once: true });
}
