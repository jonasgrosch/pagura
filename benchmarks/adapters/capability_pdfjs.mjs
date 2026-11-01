// G01 public-API glue. No competitor implementation or generated expectations.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { attachmentValues, annotationAttachmentValues, ProbeLimit } from './capability_pdfjs_services.mjs';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';

const cache = process.env.PAGURA_COMPETITORS_DIR || path.join(os.homedir(), '.cache/pagura-competitors');
const require = createRequire(path.join(path.resolve(cache), 'node/package.json'));
const pdfjs = await import(pathToFileURL(require.resolve('pdfjs-dist/legacy/build/pdf.mjs')).href);
const [operation, source] = process.argv.slice(2);
if (operation === '--version') {
  console.log(JSON.stringify({ engine: pdfjs.version, node: process.version,
    canvas: require('@napi-rs/canvas/package.json').version }));
} else {
  const root = path.join(path.resolve(cache), 'node/node_modules/pdfjs-dist');
  const loading = pdfjs.getDocument({ data: new Uint8Array(fs.readFileSync(source)),
    cMapUrl: path.join(root, 'cmaps/'), cMapPacked: true,
    standardFontDataUrl: path.join(root, 'standard_fonts/'),
    wasmUrl: path.join(root, 'wasm/'), isEvalSupported: false, verbosity: 0 });
  let response;
  try {
    let document;
    try {
      document = await loading.promise;
    } catch (error) {
      response = { status: 'rejected', reason: error.name };
    }
    if (document) {
      let value;
      if (operation === 'pages') {
        const sizes = [];
        for (let i = 1; i <= document.numPages; i++) {
          const viewport = (await document.getPage(i)).getViewport({ scale: 1 });
          sizes.push([viewport.width, viewport.height]);
        }
        value = { count: document.numPages, sizes };
      } else if (operation === 'metadata') {
        const { info } = await document.getMetadata();
        value = { Title: info.Title || '', Author: info.Author || '' };
      } else if (operation === 'attachments') {
        value = await attachmentValues(document);
      } else if (operation === 'annotation-attachments') {
        value = await annotationAttachmentValues(document, pdfjs.AnnotationType.FILEATTACHMENT);
      } else if (operation === 'text') {
        const content = await (await document.getPage(1)).getTextContent();
        value = content.items.filter(item => 'str' in item)
          .map(item => item.str + (item.hasEOL ? '\n' : '')).join('')
          .replaceAll('\r\n', '\n').replace(/^\n+|\n+$/g, '');
      } else {
        throw new Error('unknown operation');
      }
      response = { status: 'ok', value };
    }
  } catch (error) {
    response = { status: error instanceof ProbeLimit ? 'limit' : 'adapter-error', reason: error.name };
  } finally {
    await loading.destroy();
  }
  console.log(JSON.stringify({ schema: 1, ...response }));
}
