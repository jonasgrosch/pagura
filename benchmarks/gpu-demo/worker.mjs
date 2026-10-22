import {runHost} from '/web/host_runtime.mjs';
globalThis.addEventListener('message', async ({data}) => {
  if (data.type !== 'start') return;
  try {
    const response = await fetch('/engine.wasm');
    if (!response.ok) throw new Error('Build the demo WASM engine first');
    const module = await WebAssembly.compile(await response.arrayBuffer());
    await runHost(module, data.shared, (message, transfer = []) => globalThis.postMessage(message, transfer));
  } catch (error) { globalThis.postMessage({type: 'fatal', message: String(error)}); }
}, {once: true});
