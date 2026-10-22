/** Measured render stages and full frame latency, with untimed byte checks. */
import {SvgPage, pixelDifference} from './svg.mjs';
export function median(values) {
  const sorted = [...values].sort((a, b) => a - b), i = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[i] : (sorted[i - 1] + sorted[i]) / 2;
}

export function stats(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return {median: median(values), p95: sorted[Math.ceil(sorted.length * 0.95) - 1]};
}

export function assertPixels(actual, expected, context) {
  if (actual.length !== expected.length) throw new Error(`${context}: byte count differs`);
  for (let i = 0; i < actual.length; i++) if (actual[i] !== expected[i]) throw new Error(`${context}: channel ${i}, actual ${actual[i]}, expected ${expected[i]}`);
}

export function hash(bytes) {
  let n = 2166136261;
  for (const b of bytes) n = Math.imul(n ^ b, 16777619);
  return (n >>> 0).toString(16).padStart(8, '0');
}

function sameBytes(actual, expected) {
  if (actual.length !== expected.length) return false;
  for (let i = 0; i < actual.length; i++) if (actual[i] !== expected[i]) return false;
  return true;
}

export async function compare(engine, gpu, options, {samples = 9, warmups = 2, progress = () => {}, svg = true} = {}) {
  if (!Number.isInteger(samples) || samples < 1 || samples > 1000 || !Number.isInteger(warmups) || warmups < 0 || warmups > 100) throw new Error('Invalid sample count');
  const coldCpu = await engine.render(2, options);
  const initial = await engine.render(3, options);
  const svgInitial = svg ? await engine.render(4, options) : null;
  const result = {options, width: coldCpu.meta.width, height: coldCpu.meta.height, first_cpu: coldCpu.meta,
    pixel_hash: hash(coldCpu.data), samples: [], fallback: initial.meta.fallback ?? null, warmups,
    svg_fallback: svgInitial?.meta.fallback ?? null};
  let svgPage, lastSvg;
  let svgSamples = samples, svgWarmups = warmups;
  if (svgInitial && !result.svg_fallback) {
    const start = performance.now();
    svgPage = new SvgPage(svgInitial.data, svgInitial.meta, options.dpi ?? 150);
    lastSvg = await svgPage.rasterize();
    result.first_svg = {...svgInitial.meta, parse_ms: svgPage.parse_ms, browser_ms: performance.now() - start,
      frame_ms: svgInitial.host_ms + performance.now() - start};
    result.svg_snapshot_context = lastSvg.context;
    result.svg_bytes = svgInitial.data.length;
    result.svg_resources = {paths: svgInitial.meta.paths, glyphs: svgInitial.meta.glyphs, images: svgInitial.meta.images};
    if (lastSvg.wall_ms > 250) {
      svgSamples = Math.min(samples, 3);svgWarmups = Math.min(warmups, 1);
      result.svg_sample_cap = 'Initial completed SVG raster exceeded 250 ms; at most 3 timed samples and 1 warmup';
    }
  }
  let job;
  if (!result.fallback) {
    if (!gpu) result.fallback = 'No hardware WebGPU adapter is available';
    else {
      try { job = gpu.prepare(initial.data); }
      catch (error) { result.fallback = String(error); }
    }
  }
  const reference = coldCpu.data;
  const svgReference = lastSvg?.pixels;
  const firstSvgAccuracy = svgReference ? pixelDifference(svgReference, reference) : null;
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
    const vector = async () => {
      const exported = await engine.render(4, options);
      if (exported.meta.fallback) throw new Error('SVG admission changed between identical requests');
      assertPixels(exported.data, svgInitial.data, 'SVG export repeat');
      const start = performance.now();
      const page = new SvgPage(exported.data, exported.meta, options.dpi ?? 150);
      const output = await page.rasterize();
      const browser_ms = performance.now() - start;
      const repeat_difference = sameBytes(output.pixels, svgReference) ? null : pixelDifference(output.pixels, svgReference);
      const accuracy = repeat_difference ? pixelDifference(output.pixels, reference) : firstSvgAccuracy;
      return {engine: exported.meta, browser_ms, render_ms: exported.meta.engine_ms + browser_ms,
        frame_ms: exported.host_ms + browser_ms, pixels: output.pixels, page, repeat_difference, accuracy};
    };
    let lastGpu = null;
    for (let i = 0; i < warmups; i++) { await cpu(); if (job) lastGpu = await hybrid(); if (svgPage && i < svgWarmups) lastSvg = await vector(); }
    for (let i = 0; i < samples; i++) {
      progress(i, samples);
      let c, g, s;
      // Rotate and reverse the three paths to balance earlier/later positions.
      const paths = ['cpu', ...(job ? ['gpu'] : []), ...(svgPage && i < svgSamples ? ['svg'] : [])];
      const order = paths.slice(i % paths.length).concat(paths.slice(0, i % paths.length));
      if (Math.floor(i / paths.length) % 2) order.reverse();
      for (const path of order) {
        if (path === 'cpu') c = await cpu();
        else if (path === 'gpu') g = await hybrid();
        else s = await vector();
      }
      if (g) lastGpu = g;
      if (s) {lastSvg = s;svgPage = s.page;}
      result.samples.push({cpu: {...c.meta, frame_ms: c.host_ms}, gpu: g ? {
        engine: g.engine, gpu_ms: g.gpu_ms, kernel_ms: g.kernel_ms, render_ms: g.render_ms, frame_ms: g.frame_ms,
      } : null, svg: s ? {engine: s.engine, browser_ms: s.browser_ms, render_ms: s.render_ms, frame_ms: s.frame_ms,
        repeat_difference: s.repeat_difference, accuracy: s.accuracy} : null});
    }
    result.cpu_render_ms = stats(result.samples.map(s => s.cpu.engine_ms));
    result.cpu_frame_ms = stats(result.samples.map(s => s.cpu.frame_ms));
    if (svgPage) {
      const measured = result.samples.filter(s => s.svg).map(s => s.svg);
      result.svg_samples = measured.length;result.svg_warmups = svgWarmups;
      result.svg_render_ms = stats(measured.map(s => s.render_ms));
      result.svg_frame_ms = stats(measured.map(s => s.frame_ms));
      result.svg_export_ms = stats(measured.map(s => s.engine.export_ms));
      result.svg_browser_ms = stats(measured.map(s => s.browser_ms));
      result.svg_accuracy = lastSvg.accuracy;
      result.svg_repeatability = {export_byte_identical: true, pixel_byte_identical: measured.every(s => !s.repeat_difference),
        differing_samples: measured.filter(s => s.repeat_difference).length,
        max_channel_difference: Math.max(0, ...measured.map(s => s.repeat_difference?.max_channel_error ?? 0)),
        max_changed_percent: Math.max(0, ...measured.map(s => s.repeat_difference?.changed_percent ?? 0))};
      const accuracy = [firstSvgAccuracy, ...measured.map(s => s.accuracy)];
      result.svg_accuracy_range = Object.fromEntries(['beyond_one_percent', 'mean_channel_error', 'max_channel_error'].map(key =>
        [key, [Math.min(...accuracy.map(a => a[key])), Math.max(...accuracy.map(a => a[key]))]]));
      result.svg_speedup = result.cpu_render_ms.median / result.svg_render_ms.median;
    }
    if (job) {
      result.gpu_render_ms = stats(result.samples.map(s => s.gpu.render_ms));
      result.gpu_frame_ms = stats(result.samples.map(s => s.gpu.frame_ms));
      result.gpu_kernel_ms = result.samples.every(s => s.gpu.kernel_ms !== null)
        ? stats(result.samples.map(s => s.gpu.kernel_ms)) : null;
      result.gpu_complete_ms = stats(result.samples.map(s => s.gpu.gpu_ms));
      result.packet_build_ms = stats(result.samples.map(s => s.gpu.engine.execute_ms));
      result.render_speedup = result.cpu_render_ms.median / result.gpu_render_ms.median;
      result.frame_speedup = result.cpu_frame_ms.median / result.gpu_frame_ms.median;
      result.packet_bytes = job.packet_bytes; result.spans = job.spans; result.references = job.references;
      result.gpu_allocation_validation_ms = job.setup_ms;
    }
    result.cpu_gpu_byte_identical = true;
    return {result, cpu: reference, gpu: lastGpu?.pixels ?? reference, svg: lastSvg?.pixels ?? null, svgPage};
  } finally { job?.destroy(); }
}
