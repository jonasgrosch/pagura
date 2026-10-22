/** Browser-owned SVG presentation. A retained tree supports zoom without
 * another WASM export; raster checks wait for actual Canvas pixel readback. */
const namespace = 'http://www.w3.org/2000/svg';
const tags = new Set(['svg', 'defs', 'rect', 'g', 'path', 'use', 'clipPath', 'image']);
const attributes = new Set(['xmlns', 'width', 'height', 'viewBox', 'color-interpolation', 'style', 'fill', 'transform',
  'id', 'clipPathUnits', 'clip-path', 'clip-rule', 'fill-rule', 'd', 'opacity', 'href', 'preserveAspectRatio',
  'image-rendering', 'stroke', 'stroke-width', 'stroke-linecap', 'stroke-linejoin', 'stroke-miterlimit',
  'stroke-dashoffset', 'stroke-dasharray', 'vector-effect']);

async function imageReady(image) {
  const loaded = image.complete ? Promise.resolve() : new Promise((resolve, reject) => {
    image.addEventListener('load', resolve, {once: true});
    image.addEventListener('error', () => reject(new Error('SVG image failed to load')), {once: true});
  });
  await Promise.all([loaded, image.decode()]);
}

export class SvgPage {
  constructor(bytes, {width, height}, dpi) {
    const start = performance.now();
    if (bytes.length > 64 * 1024 * 1024) throw new Error('SVG exceeds the demo byte budget');
    const text = new TextDecoder('utf-8', {fatal: true}).decode(bytes);
    // Refuse before constructing a very large DOM. This is a presentation
    // budget, independent of the engine's work and memory budgets.
    let elements = 0;
    for (const char of text) if (char === '<' && ++elements > 200000) throw new Error('SVG exceeds the presentation element budget');
    const xml = new DOMParser().parseFromString(text, 'image/svg+xml');
    if (xml.querySelector('parsererror')) throw new Error('Invalid generated SVG');
    for (const element of xml.querySelectorAll('*')) {
      if (element.namespaceURI !== namespace || !tags.has(element.localName)) throw new Error('Unexpected SVG element');
      for (const attribute of element.attributes) {
        if (!attributes.has(attribute.name)) throw new Error(`Unexpected SVG attribute ${attribute.name}`);
        const value = attribute.value;
        if (attribute.name === 'href' && !/^#[cgi]\d+$/.test(value) && !(element.localName === 'image' && /^data:image\/png;base64,[A-Za-z0-9+/=]+$/.test(value))) throw new Error('External SVG resource refused');
        if (attribute.name === 'style' && !/^(?:isolation:isolate;background:white|isolation:isolate|mix-blend-mode:(?:normal|multiply|screen|overlay|darken|lighten|color-dodge|color-burn|hard-light|soft-light|difference|exclusion|hue|saturation|color|luminosity))$/.test(value)) throw new Error('Unexpected SVG style');
        if (attribute.name === 'clip-path' && !/^url\(#c\d+\)$/.test(value)) throw new Error('External SVG clip refused');
      }
    }
    if (xml.documentElement.localName !== 'svg') throw new Error('Expected an SVG root');
    // Keep the tree inert. SVG image presentation avoids mirroring every
    // mark in the live HTML document.
    this.root = xml.documentElement;
    const viewBox = this.root.getAttribute('viewBox').trim().split(/\s+/).map(Number);
    this.extent_width = viewBox[2]; this.extent_height = viewBox[3];
    if (viewBox.length !== 4 || !viewBox.every(Number.isFinite) || !(this.extent_width > 0 && this.extent_height > 0)) throw new Error('Invalid SVG extent');
    this.width = width; this.height = height; this.dpi = dpi; this.bytes = bytes.length;
    this.parse_ms = performance.now() - start;
  }

  resize(width, height, dpi) {
    if (!Number.isInteger(width) || !Number.isInteger(height) || width < 1 || height < 1 || width * height > 16 * 1024 * 1024 || !(dpi > 0 && dpi <= 600)) throw new Error('SVG dimensions exceed demo budget');
    const w = this.extent_width * dpi / 72, h = this.extent_height * dpi / 72;
    this.root.setAttribute('width', w); this.root.setAttribute('height', h);
    if (this.image) {this.image.style.width = `${w}px`;this.image.style.height = `${h}px`;}
    this.width = width; this.height = height; this.dpi = dpi;
  }

  mount(container) {
    if (!this.image) {
      this.image = new Image(); this.image.alt = 'Browser SVG page';
      this.url = URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(this.root)], {type: 'image/svg+xml'}));
      this.image.src = this.url; this.resize(this.width, this.height, this.dpi);
    }
    container.replaceChildren(this.image);
  }

  async ready() {if (this.image) await imageReady(this.image);}
  destroy() {if (this.url) URL.revokeObjectURL(this.url);this.image?.remove();this.image = null;this.url = null;}

  clone() {
    const copy = Object.assign(Object.create(SvgPage.prototype), this);
    copy.root = this.root.cloneNode(true); copy.image = null;copy.url = null;return copy;
  }

  async rasterize(width = this.width, height = this.height, dpi = this.dpi, {retained = false} = {}) {
    const start = performance.now();
    this.resize(width, height, dpi);
    // A retained image can rasterize differently from a fresh export with
    // different intrinsic dimensions. Zoom checks must sample the shown source.
    const image = retained ? this.image : new Image();
    if (!image) throw new Error('Retained SVG is not mounted');
    let url;
    try {
      if (!retained) {
        url = URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(this.root)], {type: 'image/svg+xml'}));
        image.src = url;
      }
      await imageReady(image);
      const decode_ms = performance.now() - start;
      const canvas = document.createElement('canvas'); canvas.width = width; canvas.height = height;
      const context = canvas.getContext('2d');
      context.fillStyle = 'white';context.fillRect(0, 0, width, height);
      context.drawImage(image, 0, 0, this.extent_width * dpi / 72, this.extent_height * dpi / 72);
      // Readback completes rasterization, not just image decoding or DOM work.
      const pixels = new Uint8Array(context.getImageData(0, 0, width, height).data);
      return {pixels, wall_ms: performance.now() - start, decode_ms, context: context.getContextAttributes()};
    } finally {if (url) {image.src = ''; URL.revokeObjectURL(url);}}
  }
}

/** Differences against CPU are descriptive, never treated as an oracle. */
export function pixelDifference(actual, reference) {
  if (actual.length !== reference.length || actual.length % 4) throw new Error('Different pixel dimensions');
  const histogram = new Uint32Array(256);
  let sum = 0, squared = 0, changed = 0, beyondOne = 0, max = 0;
  for (let i = 0; i < actual.length; i += 4) {
    let peak = 0;
    for (let ch = 0; ch < 4; ch++) {
      const error = Math.abs(actual[i + ch] - reference[i + ch]);
      peak = Math.max(peak, error); sum += error; squared += error * error;
    }
    histogram[peak]++; if (peak) changed++; if (peak > 1) beyondOne++; max = Math.max(max, peak);
  }
  const pixels = actual.length / 4;
  const percentile = p => {let n = 0;for (let i = 0; i < 256; i++) {n += histogram[i];if (n >= pixels * p) return i;}return 255;};
  return {pixels, changed_pixels: changed, changed_percent: changed / pixels * 100,
    beyond_one_pixels: beyondOne, beyond_one_percent: beyondOne / pixels * 100,
    max_channel_error: max, mean_channel_error: sum / actual.length, rmse: Math.sqrt(squared / actual.length),
    p95_pixel_error: percentile(0.95), p99_pixel_error: percentile(0.99)};
}

export function differencePixels(actual, reference, gain = 4) {
  const pixels = new Uint8ClampedArray(reference.length);
  for (let i = 0; i < pixels.length; i += 4) {
    for (let ch = 0; ch < 3; ch++) pixels[i + ch] = Math.min(255, Math.abs(actual[i + ch] - reference[i + ch]) * gain);
    pixels[i + 3] = 255;
  }
  return pixels;
}
