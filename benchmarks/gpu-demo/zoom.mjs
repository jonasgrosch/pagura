/** Viewer timings use animation-frame opportunities, not a claim of display
 * scanout or GPU completion. Completed pixel checks are separate. */
import {stats, assertPixels} from './measure.mjs';
import {pixelDifference} from './svg.mjs';

const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
export async function paintOpportunity() {await frame();await frame();}
export function putPixels(canvas, pixels, width, height) {
  canvas.width = width; canvas.height = height;
  canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(pixels), width, height), 0, 0);
}

/** Warm presentation: CPU/hybrid rebuild pixels; SVG retains its compiled
 * vector/image resources. No forced full-page SVG readback in this study. */
export async function presentationStudy(engine, gpu, options, vector, {samples = 12, warmups = 2} = {}) {
  const baseline = await engine.render(2, options), packet = await engine.render(3, options), svg = vector?.clone();
  const modes = ['cpu', ...(gpu && !packet.meta.fallback ? ['gpu'] : []), ...(svg ? ['svg'] : [])];
  const stage = document.createElement('div'); stage.className = 'zoom-stage';stage.setAttribute('aria-hidden', 'true');document.body.append(stage);
  const canvas = document.createElement('canvas'), measured = Object.fromEntries(modes.map(m => [m, []])), slow = {};
  try {
    if (svg) {svg.mount(stage);await svg.ready();await paintOpportunity();}
    for (let i = -warmups; i < samples; i++) {
      const order = modes.slice((i + warmups) % modes.length).concat(modes.slice(0, (i + warmups) % modes.length));
      if (i % 2) order.reverse();
      for (const mode of order) {
        if (slow[mode] && measured[mode].length >= 3) continue;
        stage.replaceChildren();await paintOpportunity();
        const start = performance.now();let job, output;
        try {
          if (mode === 'svg') {
            svg.resize(baseline.meta.width, baseline.meta.height, options.dpi);svg.mount(stage);await svg.ready();
          } else {
            const result = await engine.render(mode === 'cpu' ? 2 : 3, options);
            if (mode === 'cpu') output = result.data;
            else {job = gpu.prepare(result.data);output = (await job.render()).pixels;}
            putPixels(canvas, output, baseline.meta.width, baseline.meta.height);stage.replaceChildren(canvas);
          }
          const position = i % 2 ? 'center' : 'top';
          stage.scrollLeft = position === 'center' ? Math.max(0, (baseline.meta.width - 480) / 2) : 0;
          stage.scrollTop = position === 'center' ? Math.max(0, (baseline.meta.height - 480) / 2) : 0;
          await paintOpportunity();const frame_ms = performance.now() - start;
          if (output) assertPixels(output, baseline.data, `${mode} presentation`);
          if (frame_ms > 250) slow[mode] = true;
          if (i >= 0) measured[mode].push({position, frame_ms});
        } finally {job?.destroy();}
      }
    }
    return {samples, warmups, viewport: {width: 480, height: 480},
      timing: 'Warm recorded scene. CPU/hybrid rebuild and upload pixels; SVG reuses its export and decoded image. Request to two animation-frame opportunities in top/centre viewport, with no full-page SVG readback. Not display scanout or GPU-completion timing.',
      paths: Object.fromEntries(modes.map(mode => [mode, {sample_cap: slow[mode] ? 'Stop after a frame above 250 ms once at least 3 samples are collected' : null,
        frame_ms: stats(measured[mode].map(s => s.frame_ms)), samples: measured[mode]}]))};
  } finally {svg?.destroy();stage.remove();}
}

export async function zoomStudy(engine, gpu, options, vector, {samples = 12, warmups = 2, gestureFrames = 60} = {}) {
  const dpis = [options.dpi, Math.round(options.dpi * 1.5), options.dpi * 2];
  if (dpis.at(-1) > 600) throw new Error('Zoom exceeds demo resolution limit');
  const stage = document.createElement('div'); stage.className = 'zoom-stage';
  stage.setAttribute('aria-hidden', 'true'); document.body.append(stage);
  const canvas = document.createElement('canvas'), svg = vector?.clone();
  const references = new Map(), jobs = new Map();
  let fallback = gpu ? null : 'No hardware WebGPU adapter';
  try {
    const calibration = [];let previousFrame = await frame();
    for (let i = 0; i < 12; i++) {const now = await frame();calibration.push(now - previousFrame);previousFrame = now;}
    const refreshInterval = stats(calibration).median;
    for (const dpi of dpis) {
      const reference = await engine.render(2, {...options, dpi}); references.set(dpi, reference);
      if (gpu && !fallback) {
        const packet = await engine.render(3, {...options, dpi});
        if (packet.meta.fallback) fallback = packet.meta.fallback;
        else jobs.set(dpi, gpu.prepare(packet.data));
      }
    }
    const modes = ['cpu', ...(!fallback ? ['gpu'] : []), ...(svg ? ['svg'] : [])];
    if (svg) {svg.mount(stage);await svg.ready();await paintOpportunity();}
    const crisp = {}, gesture = {};
    const slow = {};
    for (const mode of modes) {crisp[mode] = [];gesture[mode] = null;}
    for (let i = -warmups; i < samples; i++) {
      const dpi = dpis[(Math.max(i, 0) + 1) % dpis.length];
      const reference = references.get(dpi), {width, height} = reference.meta;
      const order = modes.slice((i + warmups) % modes.length).concat(modes.slice(0, (i + warmups) % modes.length));
      if (i % 2) order.reverse();
      for (const mode of order) {
        if (slow[mode] && crisp[mode].length >= 3) continue;
        if (mode === 'svg') svg.mount(stage);else stage.replaceChildren(canvas);
        await paintOpportunity();
        const start = performance.now();
        const position = i % 2 ? 'center' : 'top';
        let ready, output;
        if (mode === 'svg') {svg.resize(width, height, dpi);ready = performance.now() - start;}
        else if (mode === 'cpu') {
          const rendered = await engine.render(2, {...options, dpi});
          putPixels(canvas, rendered.data, width, height); ready = performance.now() - start;
          output = rendered.data;
        } else {
          const packet = await engine.render(3, {...options, dpi});
          output = (await jobs.get(dpi).render(packet.data)).pixels;
          putPixels(canvas, output, width, height); ready = performance.now() - start;
        }
        stage.scrollLeft = position === 'center' ? Math.max(0, (width - 480) / 2) : 0;
        stage.scrollTop = position === 'center' ? Math.max(0, (height - 480) / 2) : 0;
        await paintOpportunity();
        const frame_ms = performance.now() - start;
        if (output) assertPixels(output, reference.data, `${mode} zoom ${dpi}`);
        if (frame_ms > 250) slow[mode] = true;
        if (i >= 0) crisp[mode].push({dpi, position, ready_ms: ready, frame_ms});
      }
    }
    for (const mode of modes) {
      const dpi = options.dpi, reference = references.get(dpi);
      if (mode === 'svg') {svg.resize(reference.meta.width, reference.meta.height, dpi);svg.mount(stage);}
      else {putPixels(canvas, reference.data, reference.meta.width, reference.meta.height);stage.replaceChildren(canvas);}
      const element = stage.firstElementChild;
      element.style.transformOrigin = 'top left';
      await paintOpportunity();
      stage.scrollLeft = Math.max(0, (reference.meta.width - 480) / 2);
      stage.scrollTop = Math.max(0, (reference.meta.height - 480) / 2);
      const intervals = [];let previous = await frame();
      for (let i = 0; i < gestureFrames; i++) {
        element.style.transform = `scale(${1 + 0.75 * Math.sin(Math.PI * i / (gestureFrames - 1))})`;
        const now = await frame(), interval = now - previous;intervals.push(interval);previous = now;
        if (interval > 250 && intervals.length >= 3) break;
      }
      element.style.transform = '';
      const baseline = refreshInterval;
      gesture[mode] = {frames: intervals.length, requested_frames: gestureFrames, position: 'center', interval_ms: stats(intervals),
        long_frames: intervals.filter(t => t > baseline * 1.5).length,
        missed_frame_estimate: intervals.reduce((n, t) => n + Math.max(0, Math.round(t / baseline) - 1), 0)};
    }
    const accuracy = [], retainedVsFresh = [];
    if (svg) for (const dpi of dpis) {
      const reference = references.get(dpi);
      const output = await svg.rasterize(reference.meta.width, reference.meta.height, dpi, {retained: true});
      accuracy.push({dpi, ...pixelDifference(output.pixels, reference.data)});
      const fresh = await svg.rasterize(reference.meta.width, reference.meta.height, dpi);
      retainedVsFresh.push({dpi, ...pixelDifference(output.pixels, fresh.pixels)});
    }
    return {dpis, samples, warmups, refresh_interval_ms: refreshInterval, viewport: {width: 480, height: 480}, gpu_fallback: fallback,
      svg_fallback: svg ? null : 'SVG unavailable for this page',
      timing: 'Request to two animation-frame opportunities in a visible clipped 480px viewport. Includes worker transport and canvas upload for CPU/hybrid; SVG changes retained vector viewport only. Not scanout or GPU-completion timing. Resources and GPU allocations warmed; no raster readback in the SVG interaction measurement.',
      crisp: Object.fromEntries(modes.map(mode => [mode, {sample_cap: slow[mode] ? 'Stop after a frame above 250 ms once at least 3 samples are collected' : null, ready_ms: stats(crisp[mode].map(s => s.ready_ms)),
        frame_ms: stats(crisp[mode].map(s => s.frame_ms)), samples: crisp[mode]}])),
      gesture, svg_accuracy: accuracy, svg_accuracy_source: 'retained image Canvas readback', svg_retained_vs_fresh: retainedVsFresh};
  } finally {for (const job of jobs.values()) job.destroy();svg?.destroy();stage.remove();}
}
