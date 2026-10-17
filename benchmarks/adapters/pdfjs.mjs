// Independent benchmark glue written against Mozilla's documented Node API.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
const configured = process.env.PAGURA_COMPETITORS_DIR || path.join(os.homedir(), '.cache/pagura-competitors');
const cache = path.resolve(configured === '~' ? os.homedir() : configured.startsWith('~/') ? path.join(os.homedir(), configured.slice(2)) : configured);
const require = createRequire(path.join(cache, 'node/package.json'));
const modulePath = require.resolve('pdfjs-dist/legacy/build/pdf.mjs');
const pdfjs = await import(pathToFileURL(modulePath).href);
if (process.argv[2] === '--version') {
  console.log(JSON.stringify({engine: pdfjs.version, node: process.version,
    canvas: require('@napi-rs/canvas/package.json').version}));
} else {
  const args = process.argv.slice(2);
  let password;
  const passwordIndex = args.indexOf('--password');
  if (passwordIndex >= 0) {
    password = args[passwordIndex + 1];
    args.splice(passwordIndex, 2);
  }
  if (![4, 5].includes(args.length)) throw new Error('expected INPUT OUTPUT DPI PAGE or INPUT OUTDIR DPI FIRST LAST');
  const [source, target, dpi, first, last = first] = args;
  if (!Number.isInteger(Number(first)) || !Number.isInteger(Number(last)) || Number(first) < 0 || Number(last) < Number(first)) {
    throw new Error('invalid page range');
  }
  const started = process.hrtime.bigint();
  const packageRoot = pathToFileURL(path.join(cache, 'node/node_modules/pdfjs-dist/'));
  const loading = pdfjs.getDocument({
    data: new Uint8Array(fs.readFileSync(source)), password,
    cMapUrl: fileURLToPath(new URL('cmaps/', packageRoot)), cMapPacked: true,
    standardFontDataUrl: fileURLToPath(new URL('standard_fonts/', packageRoot)),
    wasmUrl: fileURLToPath(new URL('wasm/', packageRoot)), isEvalSupported: false,
  });
  const document = await loading.promise;
  const indices = Array.from({length: document.numPages}, (_, i) => i);
  if (Number(last) >= indices.length) throw new Error('page range outside document');
  const opened = process.hrtime.bigint();
  const stages = [];
  for (let index = Number(first); index <= Number(last); index++) {
  const renderStarted = process.hrtime.bigint();
  const page = await document.getPage(index + 1);
  const viewport = page.getViewport({scale: Number(dpi) / 72});
  const surface = document.canvasFactory.create(Math.ceil(viewport.width), Math.ceil(viewport.height));
  await page.render({canvasContext: surface.context, viewport, background: 'rgb(255,255,255)'}).promise;
  const width = surface.canvas.width, height = surface.canvas.height;
  const rendered = process.hrtime.bigint();
  const rgba = surface.context.getImageData(0, 0, width, height).data;
  const output = args.length === 5 ? path.join(target, `page-${String(index + 1).padStart(4, '0')}.ppm`) : target;
  fs.writeFileSync(output, Buffer.concat([Buffer.from(`RGBA8 ${width} ${height} straight\n`), Buffer.from(rgba)]));
  const emitted = process.hrtime.bigint();
  stages.push({page: index, render_ns: Number(rendered - renderStarted), emit_ns: Number(emitted - rendered)});
  document.canvasFactory.destroy(surface);
  page.cleanup();
  }
  const total = process.hrtime.bigint();
  console.log(JSON.stringify({schema: 1, open_ns: Number(opened - started), pages: stages,
    total_ns: Number(total - started)}));
  await loading.destroy();
}
