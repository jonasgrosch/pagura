/** Exploratory reading costs in WASI through the browser shim in Node.
 * node web/reading_benchmark.mjs WASM_PATH > reading.json
 * DOM samples measure construction in a stand-in, excluding browser layout.
 */
import { readFileSync } from 'node:fs';
import { cpus } from 'node:os';
import { createHash } from 'node:crypto';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';
import assert from 'node:assert/strict';
import { SessionClient } from './client.mjs';
import { runHost } from './worker.mjs';
import { profileTextLayer } from './reading_profile.mjs';
import { readingWorkload } from './reading_workload.mjs';

if (!isMainThread) {
  try { await runHost(workerData.module, workerData.shared, (message, transfer = []) => parentPort.postMessage(message, transfer)); }
  catch (error) { parentPort.postMessage({ type: 'fatal', message: String(error) }); }
} else {
  const wasm = readFileSync(process.argv[2]), module = await WebAssembly.compile(wasm);
  class Thread {
    constructor() { this.listeners = new Map(); }
    addEventListener(type, fn) { this.listeners.set(type, [...this.listeners.get(type) ?? [], fn]); }
    postMessage(message) {
      this.thread = new Worker(new URL(import.meta.url), { workerData: { module, shared: message.shared }, execArgv: ['--disable-warning=ExperimentalWarning'] });
      this.thread.on('message', (data) => { for (const fn of this.listeners.get('message') ?? []) fn({ data }); });
      this.thread.on('error', (error) => { for (const fn of this.listeners.get('error') ?? []) fn(error); });
    }
    terminate() { this.stopped = this.thread?.terminate(); }
  }
  const client = new SessionClient(new Thread()), doc = readingWorkload();
  const median = (values) => [...values].sort((a,b) => a-b)[Math.floor(values.length / 2)];
  try {
    await client.hello(); const opened = await client.open(doc.bytes);
    const begin = performance.now(), text = await client.pageText(opened.document,0), firstTextMs = performance.now() - begin;
    // Fixed cells/explicit line moves give an independent copy/search oracle.
    assert.equal(text.text, doc.text);
    const samples = [], memory = [];
    for (let sample = -1; sample < 5; sample++) {
      const begin = performance.now(); let found = 0;
      for (let page = 0; page < opened.pages; page++) {
        const result = await client.search(opened.document, 'navigation', page, page+1, 0, 1024, 4);
        assert.equal(result.done,true); assert.equal(result.matches.length, doc.lines);
        found += result.matches.length;
      }
      const elapsed = performance.now() - begin;
      if (sample >= 0) samples.push({ milliseconds: elapsed, matches: found });
    }
    for (let page = 0; page < opened.pages; page++) {
      await client.pageText(opened.document, page);
      memory.push({ page, ...(await client.readingStats()), wasmBytes: client.wasmBytes });
    }
    class Element { constructor() { this.children=[]; this.style={}; } setAttribute() {} append(node) { this.children.push(node); } }
    const standIn = { createElement: (name) => name === 'canvas' ? { getContext: () => ({ measureText: () => ({width:0.6}) }) } : new Element() };
    const layers = profileTextLayer(text,{crop:[0,0,612,792],rotation:0,userUnit:1},standIn);
    await client.close(opened.document); const afterClose = await client.readingStats(); assert.equal(afterClose.textBytes,0); assert.equal(afterClose.textPages,0);
    process.stdout.write(JSON.stringify({ exploratory:true, runtime:'browser-shim-node', node:process.version, cpu:cpus()[0]?.model,
      wasmBytes:wasm.length, wasmSha256:createHash('sha256').update(wasm).digest('hex'), pages:doc.pages, characters:text.characters.length,
      firstTextMs, layer:{ runtime:'Node DOM stand-in; no browser layout',samplesMs:layers,medianMs:median(layers) },
      search:{ query:'navigation',samples,medianMs:median(samples.map((s)=>s.milliseconds)) },memory,afterClose },null,2)+'\n');
  } finally { client.destroy(); await client.worker.stopped; }
}
