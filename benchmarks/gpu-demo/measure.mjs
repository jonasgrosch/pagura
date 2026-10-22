/** Measured render stages and full frame latency, with untimed byte checks. */
export function median(values) {
  const sorted = [...values].sort((a, b) => a - b), i = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[i] : (sorted[i - 1] + sorted[i]) / 2;
}

function stats(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return {median: median(values), p95: sorted[Math.ceil(sorted.length * 0.95) - 1]};
}

export function assertPixels(actual, expected, context) {
  if (actual.length !== expected.length) throw new Error(`${context}: pixel count differs`);
  for (let i = 0; i < actual.length; i++) if (actual[i] !== expected[i]) throw new Error(`${context}: channel ${i}, GPU ${actual[i]}, CPU ${expected[i]}`);
}

export function hash(bytes) {
  let n = 2166136261;
  for (const b of bytes) n = Math.imul(n ^ b, 16777619);
  return (n >>> 0).toString(16).padStart(8, '0');
}

export async function compare(engine, gpu, options, {samples = 9, warmups = 2, progress = () => {}} = {}) {
  if (!Number.isInteger(samples) || samples < 1 || samples > 1000 || !Number.isInteger(warmups) || warmups < 0 || warmups > 100) throw new Error('Invalid sample count');
  const coldCpu = await engine.render(2, options);
  const initial = await engine.render(3, options);
  const result = {options, width: coldCpu.meta.width, height: coldCpu.meta.height, first_cpu: coldCpu.meta,
    pixel_hash: hash(coldCpu.data), samples: [], fallback: initial.meta.fallback ?? null, warmups};
  let job;
  if (!result.fallback) {
    if (!gpu) result.fallback = 'No hardware WebGPU adapter is available';
    else {
      try { job = gpu.prepare(initial.data); }
      catch (error) { result.fallback = String(error); }
    }
  }
  const reference = coldCpu.data;
  try {
    const cpu = async () => {
      const sample = await engine.render(2, options);
      assertPixels(sample.data, reference, 'CPU repeat');
      return sample;
    };
    const hybrid = async () => {
      const packet = await engine.render(3, options);
      if (packet.meta.fallback) throw new Error('GPU admission changed between identical requests');
      const output = await job.render(packet.data);
      assertPixels(output.pixels, reference, 'GPU rendering');
      return {engine: packet.meta, host_ms: packet.host_ms, gpu_ms: output.wall_ms,
        kernel_ms: output.kernel_ms, render_ms: packet.meta.engine_ms + output.wall_ms,
        frame_ms: packet.host_ms + output.wall_ms, pixels: output.pixels};
    };
    let lastGpu = null;
    for (let i = 0; i < warmups; i++) { await cpu(); if (job) lastGpu = await hybrid(); }
    for (let i = 0; i < samples; i++) {
      progress(i, samples);
      // B C C B order across adjacent samples, for paired warm conditions.
      let c, g;
      if (!job || i % 2 === 0) { c = await cpu(); if (job) g = await hybrid(); }
      else { g = await hybrid(); c = await cpu(); }
      if (g) lastGpu = g;
      result.samples.push({cpu: {...c.meta, frame_ms: c.host_ms}, gpu: g ? {
        engine: g.engine, gpu_ms: g.gpu_ms, kernel_ms: g.kernel_ms, render_ms: g.render_ms, frame_ms: g.frame_ms,
      } : null});
    }
    result.cpu_render_ms = stats(result.samples.map(s => s.cpu.engine_ms));
    result.cpu_frame_ms = stats(result.samples.map(s => s.cpu.frame_ms));
    if (job) {
      result.gpu_render_ms = stats(result.samples.map(s => s.gpu.render_ms));
      result.gpu_frame_ms = stats(result.samples.map(s => s.gpu.frame_ms));
      result.gpu_kernel_ms = stats(result.samples.map(s => s.gpu.kernel_ms ?? 0));
      result.gpu_complete_ms = stats(result.samples.map(s => s.gpu.gpu_ms));
      result.packet_build_ms = stats(result.samples.map(s => s.gpu.engine.execute_ms));
      result.render_speedup = result.cpu_render_ms.median / result.gpu_render_ms.median;
      result.frame_speedup = result.cpu_frame_ms.median / result.gpu_frame_ms.median;
      result.packet_bytes = job.packet_bytes; result.spans = job.spans; result.references = job.references;
      result.gpu_allocation_validation_ms = job.setup_ms;
    }
    result.byte_identical = true;
    return {result, cpu: reference, gpu: lastGpu?.pixels ?? reference};
  } finally { job?.destroy(); }
}
