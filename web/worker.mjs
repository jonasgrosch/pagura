import { failureReason } from './failure.mjs';
import { runHost } from './host_runtime.mjs';
export { runHost } from './host_runtime.mjs';

if (typeof globalThis.addEventListener === 'function') {
  globalThis.addEventListener('message', async ({ data }) => {
    if (data.type !== 'start') return;
    try {
      const response = await fetch(new URL('./pagura-host.wasm', import.meta.url));
      if (!response.ok) throw new Error(`Cannot load pagura-host.wasm: HTTP ${response.status}. Run cargo wasm first.`);
      const module = await WebAssembly.compile(await response.arrayBuffer());
      await runHost(module, data.shared, (message, transfer = []) => globalThis.postMessage(message, transfer));
    } catch (error) { globalThis.postMessage({ type: 'fatal', reason: failureReason(error), message: String(error) }); }
  }, { once: true });
}
