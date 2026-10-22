/** Hardware checks and exploratory timings; failures are never pixel tolerances. */
import {demo, show} from './app.mjs';
import {samples} from './samples.mjs';
import {compare, assertPixels, hash} from './measure.mjs';
import {validateProgram} from './gpu.mjs';
import {SvgPage, pixelDifference, differencePixels} from './svg.mjs';
import {zoomStudy, presentationStudy} from './zoom.mjs';

export async function run(config) {
  await demo.ready;
  const {engine, gpu} = demo;
  if (config.mode === 'ui-test') return checkUi();
  if (config.mode === 'svg-test') return checkSvgFixtures(engine);
  if (!gpu) throw new Error('Hardware WebGPU is required for this check');
  if (config.mode === 'test') return checkFixtures(engine, gpu);
  if (config.mode !== 'bench') throw new Error(`Unknown check mode ${config.mode}`);
  const inputs = config.realOnly ? [] : samples.map(s => ({label: s.label, bytes: s.make()}));
  for (const input of await (await fetch('/benchmark-inputs.json')).json()) {
    inputs.push({label: input.label, bytes: new Uint8Array(await (await fetch(input.path)).arrayBuffer())});
  }
  const cases = [], artifacts = [];
  let preview;
  for (const input of inputs) {
    await engine.open(input.bytes);
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', input.bytes));
    const input_sha256 = Array.from(digest, b => b.toString(16).padStart(2, '0')).join('');
    for (const dpi of config.dpis) {
      console.log(`Timing ${input.label} at ${dpi} DPI`);
      const output = await compare(engine, gpu, {page: 0, dpi}, {samples: config.samples, warmups: config.warmups});
      if (dpi === 150 && config.zoom) {
        console.log(`Fresh viewport study ${input.label}`);
        output.result.presentation = await presentationStudy(engine, gpu, {page: 0, dpi}, {samples: 12, warmups: 2});
        console.log(`Zoom study ${input.label}`);
        output.result.zoom = await zoomStudy(engine, gpu, {page: 0, dpi}, output.svgPage,
          {samples: config.zoomSamples, warmups: 3, gestureFrames: config.gestureFrames});
      }
      if (input === inputs[0] && dpi === 150) preview = output;
      cases.push({label: input.label, input_sha256, ...output.result});
      show(output, input.label);
      if (dpi === 150 && output.svg) {
        const prefix = input.label.toLowerCase().replace(/[^a-z0-9]+/g, '-') + '-150';
        for (const [name, pixels] of [['cpu', output.cpu], ['svg', output.svg], ['difference', differencePixels(output.svg, output.cpu)]]) {
          const canvas = document.createElement('canvas'); canvas.width = output.result.width; canvas.height = output.result.height;
          canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(pixels), canvas.width, canvas.height), 0, 0);
          artifacts.push({name: `${prefix}-${name}.png`, data_url: canvas.toDataURL('image/png')});
        }
        artifacts.push({name: `${prefix}.svg`, text: new XMLSerializer().serializeToString(output.svgPage.root)});
      }
      console.log(`${input.label} ${dpi}: CPU ${output.result.cpu_render_ms.median.toFixed(3)} ms, hybrid ${output.result.gpu_render_ms?.median.toFixed(3) ?? output.result.fallback}`);
    }
  }
  if (preview) show(preview, inputs[0].label);
  return {
    schema: 2, timestamp: new Date().toISOString(), adapter: gpu.info,
    scope: 'Exploratory shared-desktop CPU, hybrid WebGPU and browser SVG comparison. No release-gate or SVG exactness claim.',
    timing: 'Render ms excludes worker transport in all paths. CPU/hybrid include preparation and execution; hybrid includes upload, GPU completion and RGBA readback. SVG includes scene export, browser validation/parse, image decode, Canvas rasterization and completed RGBA readback. Frame ms adds worker transport. SVG DOM interaction uses the separate retained zoom study. Every CPU/hybrid sample has a byte check outside timing; SVG differences against CPU are descriptive, not an oracle or a conformance pass.',
    order: 'Rotating and reversing CPU, GPU, SVG order across samples', cases, artifacts,
    summary: cases.map(c => ({label: c.label, dpi: c.options.dpi,
      cpu_ms: c.cpu_render_ms.median, hybrid_ms: c.gpu_render_ms?.median ?? null,
      kernel_ms: c.gpu_kernel_ms?.median ?? null, speedup: c.render_speedup ?? null,
      cpu_frame_ms: c.cpu_frame_ms.median, hybrid_frame_ms: c.gpu_frame_ms?.median ?? null,
      svg_ms: c.svg_render_ms?.median ?? null, svg_frame_ms: c.svg_frame_ms?.median ?? null,
      svg_fallback: c.svg_fallback, svg_accuracy: c.svg_accuracy,
      zoom_ms: c.zoom ? Object.fromEntries(Object.entries(c.zoom.crisp).map(([name, value]) => [name, value.frame_ms.median])) : null,
      fallback: c.fallback, byte_identical: c.byte_identical})),
  };
}

async function checkUi() {
  const wait = async condition => {
    const deadline = performance.now() + 120000;
    while (!condition()) {if (performance.now() > deadline) throw new Error('UI check timed out');await new Promise(resolve => setTimeout(resolve, 25));}
    if (!document.querySelector('#error').hidden) throw new Error(document.querySelector('#error').textContent);
  };
  document.querySelector('#controls').requestSubmit();await wait(() => !demo.busy && demo.current);
  const original = demo.current.output.result, image = document.querySelector('#svg img'), source = image.src;
  await image.decode();
  const bounds = image.getBoundingClientRect();
  if (Math.abs(bounds.width / bounds.height - original.width / original.height) > 0.01) throw new Error('SVG preview aspect differs from the CPU page');
  document.querySelector('#zoom').value = '1.5';document.querySelector('#zoom').dispatchEvent(new Event('change'));
  await wait(() => !demo.busy);
  if (image.src !== source) throw new Error('Interactive SVG zoom replaced the retained vector source');
  const cpu = document.querySelector('#cpu'), gpu = document.querySelector('#gpu'), difference = document.querySelector('#difference');
  if (cpu.width <= original.width || difference.width !== cpu.width) throw new Error('Interactive zoom did not refresh resolution and differences');
  assertPixels(gpu.getContext('2d').getImageData(0,0,gpu.width,gpu.height).data, cpu.getContext('2d').getImageData(0,0,cpu.width,cpu.height).data, 'UI zoom hybrid');
  document.querySelector('#zoom-test').click();await wait(() => !demo.busy);
  if (!demo.lastZoom?.crisp.svg) throw new Error('Zoom controls did not record SVG timings');
  return {schema: 1, failures: [], aspect_preserved: true, retained_svg_source: true, gpu_zoom_identical: true,
    zoom_dimensions: {width: cpu.width, height: cpu.height}, zoom: demo.lastZoom};
}

async function checkSvgFixtures(engine) {
  const host_checks = await checkSvgHost();
  const fixtures = await (await fetch('/fixture-index.json')).json();
  const cases = [], unsupported = {}, failures = [];
  for (const [index, fixture] of fixtures.entries()) {
    if (index % 75 === 0) console.log(`SVG fixture sweep ${index}/${fixtures.length}`);
    try {
      await engine.open(new Uint8Array(await (await fetch(fixture.path)).arrayBuffer()));
      const options = {page: 0, dpi: fixture.dpi};
      const cpu = await engine.render(2, options), svg = await engine.render(4, options);
      if (svg.meta.fallback) {
        unsupported[svg.meta.fallback] = (unsupported[svg.meta.fallback] ?? 0) + 1;
        cases.push({id: fixture.id, unsupported: svg.meta.fallback});continue;
      }
      const page = new SvgPage(svg.data, svg.meta, fixture.dpi);
      const raster = await page.rasterize();
      if (fixture.id === 'content/blend-isolated-page') {
        // Independent imaging-model result: a half-opaque quarter/half/
        // three-quarter source in an isolated page, then over white. Screen
        // must not see the final white backdrop inside the page group.
        for (let i = 0; i < raster.pixels.length; i++) if (Math.abs(raster.pixels[i] - [159,191,223,255][i % 4]) > 1) throw new Error('SVG isolated page blends with the output backdrop');
      }
      const difference = pixelDifference(raster.pixels, cpu.data);
      cases.push({id: fixture.id, svg_sha256: await sha256(svg.data), cpu_sha256: await sha256(cpu.data), ...difference});
    } catch (error) {failures.push({id: fixture.id, error: String(error)});}
  }
  return {schema: 1, timestamp: new Date().toISOString(), fixtures: fixtures.length,
    admitted: cases.filter(c => !c.unsupported).length, host_checks, unsupported, failures, cases};
}

async function checkSvgHost() {
  let refused = 0;
  const wrap = body => `<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2" viewBox="0 0 2 2">${body}</svg>`;
  for (const body of ['<script/>', '<image href="https://example.com/pixel.png"/>', '<path onclick="alert(1)"/>', '<g style="background:url(https://example.com/pixel.png)"/>']) {
    try {new SvgPage(new TextEncoder().encode(wrap(body)), {width: 2, height: 2}, 72);}
    catch {refused++;}
  }
  if (refused !== 4) throw new Error('SVG presentation admitted an active or external resource');
  const page = new SvgPage(new TextEncoder().encode(wrap('<rect width="2" height="2" fill="red"/>')), {width: 2, height: 2}, 72);
  const rendered = await page.rasterize();
  assertPixels(rendered.pixels, new Uint8Array([255,0,0,255,255,0,0,255,255,0,0,255,255,0,0,255]), 'independent opaque SVG colour');
  const copy = page.clone();copy.resize(4,4,144);
  const scaled = await copy.rasterize();
  if (scaled.pixels.length !== 64 || scaled.pixels.some((b,i) => b !== [255,0,0,255][i%4])) throw new Error('SVG zoom changed an opaque aligned colour');
  return {active_or_external_resources_refused: refused, aligned_colour_pixels: 4, zoom_pixels: 16};
}

async function checkFixtures(engine, gpu) {
  const fixtures = await (await fetch('/fixture-index.json')).json();
  const cases = [], fallback = {}, failures = [];
  let admitted = 0, variants = 0;
  // No software adapter can satisfy this test.
  if (gpu.info.isFallbackAdapter || gpu.info.type === 'CPU') throw new Error('Software GPU is not a timing or correctness result');
  const kernel_checks = await checkIntegerKernel(gpu);
  for (const [index, fixture] of fixtures.entries()) {
    if (index % 75 === 0) console.log(`GPU fixture sweep ${index}/${fixtures.length}`);
    try {
      const pdf = new Uint8Array(await (await fetch(fixture.path)).arrayBuffer());
      await engine.open(pdf);
      const options = {page: 0, dpi: fixture.dpi, band: 32};
      const cpu = await engine.render(2, options);
      const program = await engine.render(3, options);
      if (program.meta.fallback) {
        fallback[program.meta.fallback] = (fallback[program.meta.fallback] ?? 0) + 1;
        cases.push({id: fixture.id, fallback: program.meta.fallback, cpu_hash: hash(cpu.data), cpu_sha256: await sha256(cpu.data)});
        continue;
      }
      admitted++;
      const baseline = cpu.data;
      const settings = [options, {...options, band: 7}];
      if (cpu.meta.width > 3 && cpu.meta.height > 3) settings.push({...options, band: 3, region: [1, 1, cpu.meta.width - 1, cpu.meta.height - 1]});
      for (const current of settings) {
        const reference = current === options ? cpu : await engine.render(2, current);
        const packet = current === options ? program : await engine.render(3, current);
        if (packet.meta.fallback) throw new Error('A region or band changed GPU admission');
        validateProgram(packet.data, gpu.device.limits);
        const job = gpu.prepare(packet.data);
        try {
          const result = await job.render();
          assertPixels(result.pixels, reference.data, fixture.id);
          if (!current.region) assertPixels(reference.data, baseline, fixture.id + ' band invariance');
          else {
            const tile = reference.data, width = cpu.meta.width;
            for (let y = 0; y < reference.meta.height; y++) {
              const start = ((y + 1) * width + 1) * 4;
              assertPixels(tile.subarray(y * reference.meta.width * 4, (y + 1) * reference.meta.width * 4), baseline.subarray(start, start + reference.meta.width * 4), fixture.id + ' tile invariance');
            }
          }
          variants++;
        } finally { job.destroy(); }
      }
      cases.push({id: fixture.id, byte_identical: true, cpu_hash: hash(cpu.data),
        cpu_sha256: await sha256(cpu.data), program_sha256: await sha256(program.data)});
    } catch (error) { failures.push({id: fixture.id, error: String(error)}); }
  }
  // Exercise complete demo cases at all reporting resolutions too.
  for (const sample of samples) {
    await engine.open(sample.make());
    for (const dpi of [72, 150, 300]) {
      const output = await compare(engine, gpu, {dpi}, {samples: 1, warmups: 0});
      const expectedFallback = sample.id === 'image' || sample.id === 'alpha';
      if (Boolean(output.result.fallback) !== expectedFallback) failures.push({id: `${sample.id}-${dpi}`, error: 'Unexpected admission decision'});
      show(output, sample.label);
    }
  }
  const report = {schema: 1, timestamp: new Date().toISOString(), adapter: gpu.info,
    fixtures: fixtures.length, admitted, gpu_variants: variants, kernel_checks, fallback, failures, cases};
  return report;
}

async function sha256(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  return Array.from(digest, b => b.toString(16).padStart(2, '0')).join('');
}

async function checkIntegerKernel(gpu) {
  // Every possible N16 word reaches final conversion, and every coverage
  // byte reaches a blend, with independent JS integer arithmetic as oracle.
  const width = 256, height = 512, bins = width / 32 * height;
  const spans = [], cells = [], expected = new Uint8Array(width * height * 4);
  const background = [0, 65535, 32768, 65535];
  const pack = (lo, hi) => (lo | hi << 16) >>> 0;
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    const i = y * width + x, word = i % 65536;
    const first = [word, 65535 - word, (word + 32768) % 65536, 65535];
    const second = [65535 - word, word, (word + 1234) % 65536, 65535];
    spans.push([y, x, 1, 0xffffffff, pack(first[0], first[1]), pack(first[2], first[3]), 0, 0]);
    // The first 256 rows check all output words; the next 256 check all
    // coverage values against every combination of high and low words.
    if (y >= 256) {
      const offset = cells.length * 4; cells.push(x);
      spans.push([y, x, 1, offset, pack(second[0], second[1]), pack(second[2], second[3]), 0, 0]);
    }
    for (let ch = 0; ch < 4; ch++) {
      const value = y < 256 ? first[ch] : Math.floor((second[ch] * x + first[ch] * (255 - x) + 127) / 255);
      expected[i * 4 + ch] = Math.floor((value + 128) / 257);
    }
  }
  const counts = new Uint32Array(bins);
  for (const [y, x] of spans) counts[y * 8 + Math.floor(x / 32)]++;
  const spansAt = 16, cellsAt = spansAt + spans.length * 8, binsAt = cellsAt + cells.length, refsAt = binsAt + bins * 2;
  const words = new Uint32Array(refsAt + spans.length);
  words.set([0x31555047, 1, width, height, 8, pack(...background.slice(0, 2)), pack(...background.slice(2)),
    spansAt, cellsAt, binsAt, refsAt, spans.length, cells.length, bins, spans.length, words.length]);
  spans.forEach((span, i) => words.set(span, spansAt + i * 8)); words.set(cells, cellsAt);
  let offset = 0;
  for (let bin = 0; bin < bins; bin++) { const n = counts[bin]; words[binsAt + bin * 2] = offset; words[binsAt + bin * 2 + 1] = offset + n; counts[bin] = offset; offset += n; }
  spans.forEach(([y, x], i) => { words[refsAt + counts[y * 8 + Math.floor(x / 32)]++] = i; });
  const job = gpu.prepare(new Uint8Array(words.buffer));
  try { assertPixels((await job.render()).pixels, expected, 'independent N16 kernel oracle'); }
  finally { job.destroy(); }
  // Invalid packets are rejected before any dispatch.
  let rejected = 0;
  for (const mutate of [w => { w[1] = 2; }, w => { w[15]--; }, w => { w[refsAt] = spans.length; }, w => { w[binsAt + 1] = 0xffffffff; }]) {
    const invalid = words.slice(); mutate(invalid);
    try { validateProgram(new Uint8Array(invalid.buffer)); }
    catch { rejected++; }
  }
  if (rejected !== 4) throw new Error('Malformed GPU program was accepted');
  return {pixels: width * height, coverage_bytes: 256, malformed_packets_rejected: rejected};
}
