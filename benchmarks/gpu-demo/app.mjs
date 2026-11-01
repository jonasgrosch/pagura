import {installDirectPanel} from './display-panel.mjs';
import {installIntegratedPanel} from './integrated-panel.mjs';
import {browserZoom} from './retained-zoom.mjs';
import {Engine} from './engine.mjs';
import {GpuBackend} from './gpu.mjs';
import {samples} from './samples.mjs';
import {compare} from './measure.mjs';
import {differencePixels, pixelDifference} from './svg.mjs';
import {paintOpportunity, putPixels, zoomStudy} from './zoom.mjs';

const $ = id => document.getElementById(id);
const ms = value => `${value.toFixed(2)} ms`;
export const demo = {engine: null, gpu: null, ready: null};
globalThis.demo = demo;
const directPanel=installDirectPanel(demo);
const integratedPanel=installIntegratedPanel(demo);
let selected = samples[0], selectedName = selected.label, pdfBytes = null;
const workloads = [...samples];
for (const sample of samples) { const option = document.createElement('option'); option.value = sample.id; option.textContent = sample.label; $('sample').append(option); }
function status(value) { $('status').textContent = value; }
function error(value) { $('error').textContent = String(value); $('error').hidden = false; status('The comparison could not complete'); }

demo.ready = (async () => {
  const [engine, gpu] = await Promise.all([Engine.create(), GpuBackend.create().catch(failure => {
    $('adapter').textContent = String(failure); return null;
  })]);
  demo.engine = engine; demo.gpu = gpu;
  for (const input of await (await fetch('/benchmark-inputs.json')).json()) {
    const sample = {id: input.path, label: input.label, load: async () => new Uint8Array(await (await fetch(input.path)).arrayBuffer())};
    workloads.push(sample);const option = document.createElement('option');option.value = sample.id;option.textContent = sample.label;$('sample').append(option);
  }
  if (gpu) $('adapter').textContent = `${gpu.info.description || gpu.info.vendor} · ${gpu.info.backend || 'WebGPU'}\nFirst GPU setup: ${ms(gpu.info.initialization_ms)}`;
  $('compare').disabled = false;
  status('Ready · choose a workload or open a PDF');
  return demo;
})();

export function show(output, label) {
  const {result, cpu, gpu, svg, svgPage} = output;
  if (demo.current?.output.svgPage !== svgPage) demo.current?.output.svgPage?.destroy();
  if (demo.current?.output.retained !== output.retained) void demo.current?.output.retained?.clear();
  demo.current = {output, label};
  for (const paper of document.querySelectorAll('.paper')) paper.classList.remove('zoomed');
  for (const [id, pixels] of [['cpu', cpu], ['gpu', gpu]]) putPixels($(id), pixels, result.width, result.height);
  $('svg').replaceChildren();
  if (svgPage) svgPage.mount($('svg'));
  else $('svg').textContent = result.svg_fallback || 'SVG unavailable';
  if (svg) putPixels($('difference'), differencePixels(svg, cpu), result.width, result.height);
  else { $('difference').width = 1; $('difference').height = 1; }
  $('cpu-label').textContent = result.retained ? 'CPU prepared redraw' : 'CPU render';
  $('cpu-summary').textContent = result.retained ? 'Prepared scene · fresh CPU pixels' : 'Preparation + coverage + composition';
  $('cpu-time').textContent = ms(result.cpu_render_ms.median);
  $('gpu-time').textContent = result.fallback ? 'CPU fallback' : ms(result.gpu_render_ms.median);
  $('svg-time').textContent = result.svg_fallback ? 'Unavailable' : ms(result.svg_render_ms.median);
  $('svg-error').textContent = result.svg_accuracy ? `${result.svg_accuracy.beyond_one_percent.toFixed(2)}%` : '—';
  $('svg-path').textContent = result.svg_fallback ? 'Unavailable' : 'Browser vector rendering';
  const raster = result.gpu_operation !== 3;
  $('identity').textContent = result.fallback ? 'WebGPU: CPU fallback' : result.cpu_gpu_byte_identical ? 'WebGPU: identical CPU bytes in this comparison' : `WebGPU: ${result.gpu_accuracy.changed_pixels} changed pixels · max ${result.gpu_accuracy.max_channel_error} levels`;
  $('gpu-label').textContent = result.retained ? 'Retained GPU render' : raster ? 'GPU coverage render' : 'Hybrid render';
  $('gpu-summary').textContent = result.retained ? 'Resident geometry · fresh GPU raster/readback' : raster ? 'Preparation + GPU coverage + readback' : 'CPU coverage + GPU + readback';
  $('gpu-stage').textContent = raster ? 'coverage' : '+ CPU coverage';
  $('gpu-path').textContent = result.fallback ? 'CPU fallback' : raster ? `WebGPU · ${result.gpu_operation === 8 ? 'shared local' : result.gpu_operation === 5 ? 'local' : 'absolute'} f32 area` : 'WebGPU · N16 integer';
  $('detail').textContent = result.fallback ? `WebGPU fallback: ${result.fallback}. This panel shows the CPU result.`
    : `WebGPU packet: ${(result.packet_bytes / 1024).toFixed(0)} KiB; ${raster ? 'geometry export' : 'coverage and packet'} ${ms(result.packet_build_ms.median)}, GPU completion and readback ${ms(result.gpu_complete_ms.median)}. Kernel ${result.gpu_kernel_ms ? ms(result.gpu_kernel_ms.median) : 'timestamp unavailable'}.`;
  if (result.gpu_geometry) $('detail').textContent += ` ${result.gpu_geometry.edges.toLocaleString()} edges; ${result.gpu_geometry.correction_cells.toLocaleString()} exact CPU correction cells; ${result.gpu_geometry.clip_runs.toLocaleString()} prepared clip runs. Experimental f32 coverage is compared with Exact CPU, not certified as exact.`;
  if (result.retained_setup) $('detail').textContent += ` Prepared CPU scene and GPU buffers are reused. GPU setup: export ${ms(result.retained_setup.packet.execute_ms)}, allocation/validation ${ms(result.retained_setup.setup_ms)}, upload ${ms(result.retained_setup.upload_ms)}. Shared sources ${output.result.gpu_geometry?.shared_sources ?? output.retained.jobs.values().next().value?.geometry?.shared_sources ?? 0}.`;
  $('frame-detail').textContent = `Worker transport included: CPU ${ms(result.cpu_frame_ms.median)}${result.gpu_frame_ms ? `, GPU ${ms(result.gpu_frame_ms.median)}` : ''}${result.svg_frame_ms ? `, SVG ${ms(result.svg_frame_ms.median)}` : ''}. ${result.samples.length} samples after ${result.warmups} warmups.`;
  $('svg-detail').textContent = result.svg_fallback ? `SVG unavailable: ${result.svg_fallback}. No partial page is displayed.`
    : `Retained SVG raster and readback ${ms(result.svg_browser_ms.median)}. First setup ${ms(result.first_svg.frame_ms)} including export ${ms(result.first_svg.export_ms)}${result.first_svg.cache_hit ? ' (host artifact reused)' : ''}. XML and source images are reused after setup. ${result.svg_samples} samples after ${result.svg_warmups} warmups${result.svg_sample_cap ? ' (slow raster sample cap)' : ''}. ${(result.svg_bytes / 1024).toFixed(0)} KiB; ${result.svg_resources.glyphs.toLocaleString()} glyph placements; ${result.svg_resources.images} image resources; ${result.svg_resources.clips_elided} inactive clips removed, ${result.svg_resources.clips_retained} retained. Against CPU: ${result.svg_accuracy.changed_percent.toFixed(2)}% changed pixels, max channel difference ${result.svg_accuracy.max_channel_error}, mean ${result.svg_accuracy.mean_channel_error.toFixed(3)}. SVG repeat variation: ${result.svg_repeatability.differing_samples}/${result.svg_samples} samples, max channel delta ${result.svg_repeatability.max_channel_difference}. This is a differential comparison, not an exactness proof.`;
  $('empty')?.remove();
  const tr = document.createElement('tr');
  for (const text of [label, result.options.dpi, ms(result.cpu_render_ms.median), result.fallback ? 'CPU fallback' : ms(result.gpu_render_ms.median), result.svg_fallback ? result.svg_fallback : ms(result.svg_render_ms.median), result.svg_accuracy ? `${result.svg_accuracy.beyond_one_percent.toFixed(2)}% / ${result.svg_accuracy.max_channel_error}` : '—']) {
    const td = document.createElement('td'); td.textContent = text; tr.append(td);
  }
  $('history').prepend(tr);
  $('zoom').value = '1'; $('zoom-test').disabled = false;$('export-svg').disabled = !svgPage;
  $('zoom-detail').textContent = 'Zoom retains SVG geometry; CPU and GPU rebuild pixels at the new resolution. Measure zoom for frame timing.';
  status(`${label} · ${result.width} × ${result.height} pixels · comparison complete`);
  directPanel.enable();
  integratedPanel.enable();
}

async function changeZoom() {
  if (!demo.current || demo.busy) return;
  const {output} = demo.current, {result, svgPage} = output;
  const factor = Number($('zoom').value), dpi = Math.round(result.options.dpi * factor);
  demo.busy = true;
  $('zoom').disabled = true; $('compare').disabled = true; $('zoom-test').disabled = true;
  try {
    const svgStart = performance.now();
    if (svgPage) svgPage.resize(Math.ceil(result.width * factor), Math.ceil(result.height * factor), dpi);
    for (const paper of document.querySelectorAll('.paper')) paper.classList.toggle('zoomed', factor > 1);
    // Ceiling at the page edge follows the same global device grid.
    const dimensions = await demo.engine.render(result.retained ? 7 : 2, {...result.options, dpi});
    if (svgPage) svgPage.resize(dimensions.meta.width, dimensions.meta.height, dpi);
    const cpuStart = performance.now();
    putPixels($('cpu'), dimensions.data, dimensions.meta.width, dimensions.meta.height);
    await paintOpportunity();
    const cpuFrame = dimensions.host_ms + performance.now() - cpuStart;
    let gpuFrame, gpuPixels = dimensions.data, zoomFallback = result.fallback;
    if (!result.fallback && demo.gpu) {
      const start = performance.now();
      if (output.retained) {
        const rendered = await output.retained.render({...result.options,dpi});
        if (rendered.fallback) {zoomFallback=rendered.fallback;putPixels($('gpu'),dimensions.data,dimensions.meta.width,dimensions.meta.height);}
        else {gpuPixels=rendered.pixels;putPixels($('gpu'),gpuPixels,dimensions.meta.width,dimensions.meta.height);await paintOpportunity();gpuFrame=performance.now()-start;}
      } else {
      const packet = await demo.engine.render(result.gpu_operation, {...result.options, dpi});
      if (!packet.meta.fallback) {
        const job = demo.gpu.prepare(packet.data);
        try { const pixels = await job.render(packet.data);gpuPixels = pixels.pixels;putPixels($('gpu'), pixels.pixels, dimensions.meta.width, dimensions.meta.height);await paintOpportunity();gpuFrame = performance.now() - start; }
        finally {job.destroy();}
      } else {zoomFallback = packet.meta.fallback;putPixels($('gpu'), dimensions.data, dimensions.meta.width, dimensions.meta.height);}
      }
    } else putPixels($('gpu'), dimensions.data, dimensions.meta.width, dimensions.meta.height);
    const delta = pixelDifference(gpuPixels, dimensions.data);
    $('identity').textContent = zoomFallback ? `WebGPU: CPU fallback at ${dpi} DPI` : `WebGPU at ${dpi} DPI: ${delta.changed_pixels} changed pixels · max ${delta.max_channel_error} levels`;
    if (svgPage) {
      const raster = await svgPage.rasterize(dimensions.meta.width, dimensions.meta.height, dpi, {retained: true});
      putPixels($('difference'), differencePixels(raster.pixels, dimensions.data), dimensions.meta.width, dimensions.meta.height);
      $('svg-error').textContent = `${pixelDifference(raster.pixels, dimensions.data).beyond_one_percent.toFixed(2)}%`;
    }
    $('zoom-detail').textContent = `At ${dpi} DPI: CPU ${ms(cpuFrame)}${gpuFrame ? `; GPU ${ms(gpuFrame)}` : ''}. SVG reused its vector tree. Use “Measure zoom” for isolated setup and retained frame timings. Preview panels share CPU preparation at a new DPI. Total update ${ms(performance.now() - svgStart)}.`;
  } catch (failure) {error(failure);}
  finally {demo.busy = false;$('zoom').disabled = false;$('compare').disabled = false;$('zoom-test').disabled = false;}
}

$('export-svg').addEventListener('click', () => {
  const page = demo.current?.output.svgPage;if (!page?.url) return;
  const link = document.createElement('a');link.href = page.url;
  link.download = `page-${demo.current.output.result.options.page + 1}.svg`;
  document.body.append(link);link.click();link.remove();
});

$('zoom').addEventListener('change', changeZoom);
$('zoom-test').addEventListener('click', async () => {
  if (!demo.current || demo.busy) return;
  demo.busy = true; $('zoom-test').disabled = true; $('compare').disabled = true; $('zoom').disabled = true;
  try {
    status('Measuring retained zoom and fresh frames…');
    const {output} = demo.current;
    const study = output.retained ? await browserZoom(demo.engine,demo.gpu,output.result.options,{retained:output.retained}) : await zoomStudy(demo.engine, demo.gpu, output.result.options, output.svgPage, {operation: output.result.gpu_operation});
    demo.lastZoom = study;
    if (output.retained) $('zoom-detail').textContent = `New resolution: CPU ${ms(study.cold.cpu.frame_ms.median)}, GPU ${study.cold.gpu ? ms(study.cold.gpu.frame_ms.median) : study.fallback}. Retained resolutions: CPU ${ms(study.warm.cpu.frame_ms.median)}, GPU ${study.warm.gpu ? ms(study.warm.gpu.frame_ms.median) : study.fallback}. Includes completed pixels and two animation-frame opportunities; not scanout. CPU and GPU each keep at most three resolutions.`;
    else $('zoom-detail').textContent = Object.entries(study.crisp).map(([mode, timing]) => `${mode.toUpperCase()} ${ms(timing.frame_ms.median)} (p95 ${ms(timing.frame_ms.p95)})`).join(' · ') + '. Request to animation-frame opportunity; not display scanout. Retained CSS gesture intervals: ' + Object.entries(study.gesture).map(([mode, g]) => `${mode} ${ms(g.interval_ms.median)}`).join(' · ');
    status('Zoom comparison complete');
  } catch (failure) {error(failure);}
  finally {demo.busy = false;$('zoom-test').disabled = false;$('compare').disabled = false;$('zoom').disabled = false;}
});

async function run() {
  if (demo.busy) return;
  demo.busy = true; $('compare').disabled = true; $('zoom-test').disabled = true; $('zoom').disabled = true; $('error').hidden = true;
  try {
    await directPanel.clear();
    await integratedPanel.clear();
    await demo.current?.output.retained?.clear();
    await demo.ready;
    const pdf = pdfBytes ?? (selected.make ? selected.make() : await selected.load());
    demo.input=pdf;
    const opened = await demo.engine.open(pdf);
    $('page').max = opened.meta.pages;
    const page = Number($('page').value) - 1;
    if (!Number.isInteger(page) || page < 0 || page >= opened.meta.pages) throw new Error('Choose an existing page');
    status(`Comparing ${selectedName}…`);
    const output = await compare(demo.engine, demo.gpu, {page, dpi: Number($('dpi').value)}, {
      operation: Number($('gpu-mode').value),
      progress: (i, n) => status(`Measuring ${selectedName} · ${i + 1} / ${n}`),
    });
    show(output, selectedName);
  } catch (failure) { error(failure); }
  finally { demo.busy = false; $('compare').disabled = false; $('zoom').disabled = false; $('zoom-test').disabled = !demo.current; }
}

$('controls').addEventListener('submit', event => { event.preventDefault(); void run(); });
$('sample').addEventListener('change', () => {
  selected = workloads.find(sample => sample.id === $('sample').value); selectedName = selected.label; pdfBytes = null; $('file').value = ''; $('page').value = '1';
});
$('file').addEventListener('change', async () => {
  const file = $('file').files[0]; if (!file) return;
  if (file.size > 64 * 1024 * 1024 - 4) { error('PDF exceeds the 64 MiB demo limit'); return; }
  pdfBytes = new Uint8Array(await file.arrayBuffer()); selectedName = file.name; $('page').value = '1'; status(`Ready to compare ${file.name}`);
});
if (!new URL(location.href).searchParams.has('runner')) demo.ready.then(run).catch(error);
else demo.ready.catch(error);
