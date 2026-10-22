import {Engine} from './engine.mjs';
import {GpuBackend} from './gpu.mjs';
import {samples} from './samples.mjs';
import {compare} from './measure.mjs';

const $ = id => document.getElementById(id);
const ms = value => `${value.toFixed(2)} ms`;
export const demo = {engine: null, gpu: null, ready: null};
globalThis.demo = demo;
let selected = samples[0], selectedName = selected.label, pdfBytes = null;
for (const sample of samples) { const option = document.createElement('option'); option.value = sample.id; option.textContent = sample.label; $('sample').append(option); }
function status(value) { $('status').textContent = value; }
function error(value) { $('error').textContent = String(value); $('error').hidden = false; status('The comparison could not complete'); }

demo.ready = (async () => {
  const [engine, gpu] = await Promise.all([Engine.create(), GpuBackend.create().catch(failure => {
    $('adapter').textContent = String(failure); return null;
  })]);
  demo.engine = engine; demo.gpu = gpu;
  if (gpu) $('adapter').textContent = `${gpu.info.description || gpu.info.vendor} · ${gpu.info.backend || 'WebGPU'}\nFirst GPU setup: ${ms(gpu.info.initialization_ms)}`;
  $('compare').disabled = false;
  status('Ready · choose a workload or open a PDF');
  return demo;
})();

export function show({result, cpu, gpu}, label) {
  for (const [id, pixels] of [['cpu', cpu], ['gpu', gpu]]) {
    const canvas = $(id); canvas.width = result.width; canvas.height = result.height;
    // The demo uses an opaque white background, so RGBA is already straight.
    canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(pixels), result.width, result.height), 0, 0);
  }
  $('cpu-time').textContent = ms(result.cpu_render_ms.median);
  $('gpu-time').textContent = result.fallback ? 'CPU fallback' : ms(result.gpu_render_ms.median);
  $('speedup').textContent = result.fallback ? '—' : `${result.render_speedup.toFixed(2)}×`;
  $('speed-caption').textContent = result.fallback ? 'Same CPU renderer' : result.render_speedup >= 1 ? 'Hybrid is faster on this workload' : 'CPU is faster on this workload';
  $('kernel-time').textContent = result.fallback ? '—' : ms(result.gpu_kernel_ms.median);
  $('identity').textContent = result.fallback ? 'Exact CPU fallback' : '✓ Byte-identical to CPU';
  $('gpu-path').textContent = result.fallback ? 'CPU fallback' : 'WebGPU · N16 integer';
  $('detail').textContent = result.fallback ? `CPU fallback: ${result.fallback}. Both previews show the exact CPU result.`
    : `The GPU program contains ${result.spans.toLocaleString()} exact spans, ${(result.packet_bytes / 1024).toFixed(0)} KiB to upload. Packet construction takes ${ms(result.packet_build_ms.median)}; GPU upload, execution and readback take ${ms(result.gpu_complete_ms.median)}.`;
  $('frame-detail').textContent = result.fallback ? `Measured ${result.samples.length} warm CPU renders, after ${result.warmups} warmups.`
    : `Full frame latency including the worker transport: CPU ${ms(result.cpu_frame_ms.median)}, hybrid ${ms(result.gpu_frame_ms.median)}. ${result.samples.length} samples after ${result.warmups} warmups. One-time GPU setup ${ms(demo.gpu.info.initialization_ms)}; program validation and buffer allocation ${ms(result.gpu_allocation_validation_ms)}.`;
  $('empty')?.remove();
  const tr = document.createElement('tr');
  for (const text of [label, result.options.dpi, ms(result.cpu_render_ms.median), result.fallback ? 'CPU fallback' : ms(result.gpu_render_ms.median), result.fallback ? '—' : `${result.render_speedup.toFixed(2)}×`, result.fallback ? result.fallback : 'Byte-identical']) {
    const td = document.createElement('td'); td.textContent = text; tr.append(td);
  }
  $('history').prepend(tr);
  status(`${label} · ${result.width} × ${result.height} pixels · ${result.fallback ? 'CPU fallback' : 'comparison complete'}`);
}

async function run() {
  $('compare').disabled = true; $('error').hidden = true;
  try {
    await demo.ready;
    const pdf = pdfBytes ?? selected.make();
    const opened = await demo.engine.open(pdf);
    $('page').max = opened.meta.pages;
    const page = Number($('page').value) - 1;
    if (!Number.isInteger(page) || page < 0 || page >= opened.meta.pages) throw new Error('Choose an existing page');
    status(`Comparing ${selectedName}…`);
    const output = await compare(demo.engine, demo.gpu, {page, dpi: Number($('dpi').value)}, {
      progress: (i, n) => status(`Measuring ${selectedName} · ${i + 1} / ${n}`),
    });
    show(output, selectedName);
  } catch (failure) { error(failure); }
  finally { $('compare').disabled = false; }
}

$('controls').addEventListener('submit', event => { event.preventDefault(); void run(); });
$('sample').addEventListener('change', () => {
  selected = samples.find(sample => sample.id === $('sample').value); selectedName = selected.label; pdfBytes = null; $('file').value = ''; $('page').value = '1';
});
$('file').addEventListener('change', async () => {
  const file = $('file').files[0]; if (!file) return;
  if (file.size > 64 * 1024 * 1024 - 4) { error('PDF exceeds the 64 MiB demo limit'); return; }
  pdfBytes = new Uint8Array(await file.arrayBuffer()); selectedName = file.name; $('page').value = '1'; status(`Ready to compare ${file.name}`);
});
if (!new URL(location.href).searchParams.has('runner')) demo.ready.then(run).catch(error);
else demo.ready.catch(error);
