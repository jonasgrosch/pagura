/** Experimental exact integer N16 backend. No Rust GPU dependency or FFI. */
export function validateProgram(buffer, limits) {
  if (!(buffer instanceof Uint8Array) || buffer.byteLength < 64 || buffer.byteLength % 4) throw new Error('Truncated GPU program');
  // The engine's wire representation is explicitly little endian.
  const view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
  const words = new Uint32Array(buffer.byteLength / 4);
  for (let i = 0; i < words.length; i++) words[i] = view.getUint32(i * 4, true);
  const [magic, version, width, height, columns] = words;
  if (magic !== 0x31555047 || version !== 1 || !width || !height || columns !== Math.ceil(width / 32)) throw new Error('Invalid GPU program header');
  if (width * height > 16 * 1024 * 1024 || words[15] !== words.length) throw new Error('GPU program size exceeds demo budget');
  const [spansAt, cellsAt, binsAt, refsAt, spanCount, cellWords, bins, refs] = words.subarray(7, 15);
  if (spansAt !== 16 || cellsAt !== spansAt + spanCount * 8 || binsAt !== cellsAt + cellWords || refsAt !== binsAt + bins * 2
      || bins !== columns * height || words.length !== refsAt + refs) throw new Error('Invalid GPU program sections');
  if (buffer.byteLength > 64 * 1024 * 1024) throw new Error('GPU packet exceeds 64 MiB');
  if (limits && (buffer.byteLength > limits.maxStorageBufferBindingSize || width * height * 4 > limits.maxStorageBufferBindingSize
      || Math.ceil(width / 64) > limits.maxComputeWorkgroupsPerDimension || height > limits.maxComputeWorkgroupsPerDimension)) throw new Error('GPU program exceeds device limits');
  let previous = 0;
  for (let bin = 0; bin < bins; bin++) {
    const start = words[binsAt + bin * 2], end = words[binsAt + bin * 2 + 1];
    if (start !== previous || end < start || end > refs) throw new Error('Invalid GPU bin range');
    let last = -1;
    for (let i = start; i < end; i++) {
      const id = words[refsAt + i];
      if (id >= spanCount || id <= last) throw new Error('GPU bin is out of source order');
      last = id;
      const at = spansAt + id * 8, y = words[at], x = words[at + 1], n = words[at + 2];
      if (y !== Math.floor(bin / columns) || x >= (bin % columns + 1) * 32 || x + n <= bin % columns * 32) throw new Error('GPU span is in the wrong bin');
    }
    previous = end;
  }
  if (previous !== refs) throw new Error('Incomplete GPU bins');
  for (let i = 0; i < spanCount; i++) {
    const at = spansAt + i * 8, [y, x, n, coverage] = words.subarray(at, at + 4);
    if (y >= height || !n || x >= width || n > width - x || coverage !== 0xffffffff && coverage + n > cellWords * 4) throw new Error('Invalid GPU span');
  }
  return {words, width, height, spans: spanCount, references: refs};
}

export class GpuBackend {
  static async create() {
    if (!navigator.gpu) throw new Error('WebGPU is unavailable in this browser');
    const start = performance.now();
    const adapter = await navigator.gpu.requestAdapter({powerPreference: 'high-performance'});
    if (!adapter || adapter.info.isFallbackAdapter) throw new Error('No hardware WebGPU adapter is available');
    const timestamps = adapter.features.has('timestamp-query');
    const device = await adapter.requestDevice({requiredFeatures: timestamps ? ['timestamp-query'] : []});
    let lost = null;
    device.lost.then(info => { lost = info.message || 'GPU device lost'; });
    const response = await fetch(new URL('./composite.wgsl', import.meta.url));
    if (!response.ok) throw new Error('Cannot load GPU shader');
    const module = device.createShaderModule({code: await response.text()});
    const errors = (await module.getCompilationInfo()).messages.filter(message => message.type === 'error');
    if (errors.length) { device.destroy(); throw new Error(errors.map(error => error.message).join('\n')); }
    const pipeline = await device.createComputePipelineAsync({layout: 'auto', compute: {module, entryPoint: 'composite'}});
    const info = adapter.info;
    return new GpuBackend(device, pipeline, timestamps, {
      vendor: info.vendor, architecture: info.architecture, device: info.device, description: info.description,
      isFallbackAdapter: info.isFallbackAdapter, backend: info.backend ?? null, type: info.type ?? null,
      userAgent: navigator.userAgent, initialization_ms: performance.now() - start,
    }, () => lost);
  }

  constructor(device, pipeline, timestamps, info, lost) {
    Object.assign(this, {device, pipeline, timestamps, info, lost});
  }

  /** Validation, allocation and initial upload are measured separately. */
  prepare(bytes) {
    if (this.lost()) throw new Error(this.lost());
    const start = performance.now();
    const program = validateProgram(bytes, this.device.limits);
    const {device, pipeline, timestamps} = this;
    const source = device.createBuffer({size: bytes.length, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST});
    const size = program.width * program.height * 4;
    const output = device.createBuffer({size, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC});
    const readback = device.createBuffer({size, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST});
    const group = device.createBindGroup({layout: pipeline.getBindGroupLayout(0), entries: [
      {binding: 0, resource: {buffer: source}}, {binding: 1, resource: {buffer: output}},
    ]});
    const queries = timestamps ? device.createQuerySet({type: 'timestamp', count: 2}) : null;
    const resolve = timestamps ? device.createBuffer({size: 16, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC}) : null;
    const timing = timestamps ? device.createBuffer({size: 16, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST}) : null;
    const setup_ms = performance.now() - start;
    let busy = false, destroyed = false;
    return {
      setup_ms, width: program.width, height: program.height, spans: program.spans,
      packet_bytes: bytes.length, references: program.references,
      render: async (upload = bytes) => {
        if (busy || destroyed || this.lost()) throw new Error(this.lost() || 'GPU job is unavailable');
        if (!(upload instanceof Uint8Array) || upload.length !== bytes.length) throw new Error('GPU program changed; prepare a new job');
        busy = true;
        device.pushErrorScope('validation');
        let scoped = true;
        const started = performance.now();
        try {
          // Every measured fresh render uploads its program; residency is not
          // silently assumed. Reusing allocations matches the CPU Scratch.
          device.queue.writeBuffer(source, 0, upload);
          const encoder = device.createCommandEncoder();
          const pass = encoder.beginComputePass(queries ? {timestampWrites: {
            querySet: queries, beginningOfPassWriteIndex: 0, endOfPassWriteIndex: 1,
          }} : {});
          pass.setPipeline(pipeline); pass.setBindGroup(0, group);
          pass.dispatchWorkgroups(Math.ceil(program.width / 64), program.height);
          pass.end();
          encoder.copyBufferToBuffer(output, 0, readback, 0, size);
          if (queries) {
            encoder.resolveQuerySet(queries, 0, 2, resolve, 0);
            encoder.copyBufferToBuffer(resolve, 0, timing, 0, 16);
          }
          device.queue.submit([encoder.finish()]);
          // Completion includes upload, dispatch, RGBA readback and the copy
          // into JS-owned memory. Submission time alone is never a timing.
          await readback.mapAsync(GPUMapMode.READ);
          const pixels = new Uint8Array(readback.getMappedRange()).slice();
          readback.unmap();
          const wall_ms = performance.now() - started;
          let kernel_ms = null;
          if (timing) {
            await timing.mapAsync(GPUMapMode.READ);
            const times = new BigUint64Array(timing.getMappedRange());
            kernel_ms = Number(times[1] - times[0]) / 1e6;
            timing.unmap();
          }
          const error = await device.popErrorScope();
          scoped = false;
          if (error) throw new Error(error.message);
          if (this.lost()) throw new Error(this.lost());
          return {pixels, wall_ms, kernel_ms};
        } catch (error) {
          if (readback.mapState === 'mapped') readback.unmap();
          if (timing?.mapState === 'mapped') timing.unmap();
          if (scoped) await device.popErrorScope().catch(() => {});
          throw error;
        } finally { busy = false; }
      },
      destroy() {
        destroyed = true;
        for (const resource of [source, output, readback, queries, resolve, timing]) resource?.destroy();
      },
    };
  }

  destroy() { this.device.destroy(); }
}
