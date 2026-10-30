import {runHost} from '/web/host_runtime.mjs';
globalThis.addEventListener('message', async ({data}) => {
  if (data.type !== 'start') return;
  try {
    const response = await fetch('/engine.wasm');
    if (!response.ok) throw new Error('Build the demo WASM engine first');
    const module = await WebAssembly.compile(await response.arrayBuffer());
    // The experiment retains recorded, prepared and exported geometry at once.
    // This aggregate cap is separate from the 512 MiB preparation meter.
    await runHost(module, data.shared, (message, transfer = []) => globalThis.postMessage(message, transfer), {memoryBytes: 1024 * 1024 * 1024});
  } catch (error) { globalThis.postMessage({type: 'fatal', message: String(error)}); }
}, {once: true});
