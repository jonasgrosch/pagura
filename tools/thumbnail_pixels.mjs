/** Real Canvas pixels, compared with an independently drawn, untiled source.
 * Run through thumbnail_pixels.py on a hosted runner; no WASM build required. */
import { thumbnailRaster } from '../web/thumbnail_raster.mjs';

const check = (condition, message) => { if (!condition) throw new Error(message); };
function canvas(width, height) {
  const result = document.createElement('canvas');
  result.width = width; result.height = height;
  return result;
}
function artwork(width, height, kind) {
  const result = canvas(width, height), context = result.getContext('2d');
  if (kind !== 'transparent') {
    context.fillStyle = '#192d50'; context.fillRect(0, 0, width, height);
  }
  if (kind !== 'solid') {
    context.fillStyle = kind === 'transparent' ? '#e3942866' : '#efefef';
    context.font = '19px sans-serif'; context.lineWidth = 1.25;
    context.strokeStyle = kind === 'transparent' ? '#1385dacc' : '#2088ff';
    // Text, diagonals and circles span both sides of every future tile edge.
    for (let y = 241; y < height; y += 256) {
      for (let x = 218; x < width; x += 256) {
        context.fillText('Across tile edges', x, y + 17);
        context.beginPath(); context.arc(x + 38, y + 15, 37, 0, 2 * Math.PI); context.stroke();
      }
    }
    for (let x = -height; x < width; x += 31) {
      context.beginPath(); context.moveTo(x, 0); context.lineTo(x + height, height); context.stroke();
    }
  }
  return result;
}
function split(source) {
  const tiles = new Map();
  for (let y = 0; y < source.height; y += 256) for (let x = 0; x < source.width; x += 256) {
    const tile = canvas(Math.min(256, source.width - x), Math.min(256, source.height - y));
    tile.getContext('2d').drawImage(source, x, y, tile.width, tile.height, 0, 0, tile.width, tile.height);
    tiles.set(`${x},${y}`, tile);
  }
  return { width: source.width, height: source.height, tiles };
}
function reference(source, width, height) {
  const result = canvas(width, height), context = result.getContext('2d');
  context.imageSmoothingEnabled = true; context.imageSmoothingQuality = 'high';
  context.drawImage(source, 0, 0, width, height);
  return result;
}
function pixels(source) { return source.getContext('2d').getImageData(0, 0, source.width, source.height).data; }
function differences(a, b) {
  check(a.length === b.length, 'pixel buffer lengths differ');
  let count = 0;
  for (let i = 0; i < a.length; i += 4) {
    if (a[i] !== b[i] || a[i + 1] !== b[i + 1] || a[i + 2] !== b[i + 2] || a[i + 3] !== b[i + 3]) count++;
  }
  return count;
}
function trackedDocument() {
  const allocated = [];
  return {
    allocated,
    createElement(tag) {
      check(tag === 'canvas', 'unexpected allocation');
      const value = canvas(0, 0); allocated.push(value); return value;
    },
    released(output = null) {
      for (const value of allocated) if (value !== output) check(value.width === 0 && value.height === 0, 'scratch retained');
    },
  };
}
function release(source, raster) {
  source.width = source.height = 0;
  for (const tile of raster?.tiles.values() ?? []) tile.width = tile.height = 0;
}

export async function run() {
  const results = [], images = {};
  const test = async (name, action) => {
    try { results.push({ name, passed: true, ...await action() }); }
    catch (error) { results.push({ name, passed: false, error: error.stack || String(error) }); }
  };
  for (const [width, height] of [[913, 707], [601, 803], [1025, 257]]) {
    for (const kind of ['solid', 'strokes', 'transparent']) {
      await test(`${width}x${height}-${kind}`, async () => {
        const source = artwork(width, height, kind), raster = split(source), doc = trackedDocument();
        const originals = [...raster.tiles.values()].map(tile => pixels(tile));
        let renders = 0;
        const actual = await thumbnailRaster({ doc, width, height, raster, current: () => true,
          render: () => { renders++; throw new Error('complete cache must be reused'); } });
        const scale = Math.min(280 / width, 332 / height);
        check(actual.width === Math.ceil(width * scale - 1e-9) && actual.height === Math.ceil(height * scale - 1e-9), 'wrong thumbnail dimensions');
        check(renders === 0, 'cache reuse rendered new tiles');
        const expected = reference(source, actual.width, actual.height);
        const changedPixels = differences(pixels(actual), pixels(expected));
        if (changedPixels || kind === 'strokes' && width === 913) {
          images[`${width}-${kind}-actual`] = actual.toDataURL();
          images[`${width}-${kind}-reference`] = expected.toDataURL();
        }
        doc.released(actual);
        for (const [i, tile] of [...raster.tiles.values()].entries()) {
          check(differences(pixels(tile), originals[i]) === 0, 'cached tile changed');
        }
        check(changedPixels === 0, `${changedPixels} pixels differ from untiled downsample`);
        if (kind === 'solid') {
          const bytes = pixels(actual);
          for (let i = 0; i < bytes.length; i += 4) check(bytes[i] === 25 && bytes[i + 1] === 45 && bytes[i + 2] === 80 && bytes[i + 3] === 255, 'solid background has a seam');
        }
        if (kind === 'strokes') {
          // Uniform fills can hide the old bug in some Canvas backends. Fine
          // artwork across fractional tile edges must expose separate filtering.
          const legacy = canvas(actual.width, actual.height), context = legacy.getContext('2d');
          context.imageSmoothingEnabled = true; context.imageSmoothingQuality = 'high';
          for (const [key, tile] of raster.tiles) {
            const [x, y] = key.split(',').map(Number);
            context.drawImage(tile, x * actual.width / width, y * actual.height / height,
              tile.width * actual.width / width, tile.height * actual.height / height);
          }
          const legacyDifferences = differences(pixels(legacy), pixels(expected));
          check(legacyDifferences > 0, 'negative control failed to expose the original seams');
          if (width === 913) {
            images['seams-before'] = legacy.toDataURL(); images['seams-after'] = actual.toDataURL();
          }
          results.push({ name: `${width}x${height}-old-path-negative-control`, passed: true, changedPixels: legacyDifferences });
          release(legacy);
        }
        release(source, raster); release(actual); release(expected);
        return { changedPixels };
      });
    }
  }
  for (const [name, raster] of [
    ['area-limit', { width: 2048, height: 2048, tiles: { get() { throw new Error('oversized cache inspected'); } } }],
    ['axis-limit', { width: 8192, height: 128, tiles: { get() { throw new Error('oversized cache inspected'); } } }],
    ['incomplete-cache', { width: 913, height: 707, tiles: new Map() }],
  ]) await test(name, async () => {
    const width = 913, height = 707, scale = Math.min(280 / width, 332 / height) * 4;
    const source = artwork(Math.ceil(width * scale - 1e-9), Math.ceil(height * scale - 1e-9), 'strokes');
    const doc = trackedDocument(); let chunks = 0;
    const actual = await thumbnailRaster({ doc, width, height, raster, current: () => true, render: async (renderScale, tile) => {
      check(renderScale === scale && tile.width <= 256 && tile.height <= 256, 'unbounded fallback tile');
      check(doc.allocated[0].width * doc.allocated[0].height <= 1120 * 1328, 'fallback scratch exceeds bound');
      chunks++;
      const data = source.getContext('2d').getImageData(tile.x, tile.y, tile.width, tile.height);
      return { width: tile.width, height: tile.height, rgba: data.data };
    } });
    check(chunks > 1, 'fallback did not render chunks'); doc.released(actual);
    const expected = reference(source, actual.width, actual.height);
    check(differences(pixels(actual), pixels(expected)) === 0, 'fallback pixels differ');
    release(source); release(actual); release(expected);
    return { chunks };
  });
  for (const eviction of [false, true]) await test(eviction ? 'cache-eviction-during-yield' : 'foreground-interrupt', async () => {
    const source = artwork(1024, 2048, 'solid'), raster = split(source), doc = trackedDocument();
    let current = true, yielded = false, boundaryObserved = false;
    setTimeout(() => {
      yielded = true;
      boundaryObserved = doc.allocated[0].width * doc.allocated[0].height === 2 * 1024 * 1024;
      if (eviction) raster.tiles.get('0,256').width = 0; else current = false;
    }, 0);
    const actual = await thumbnailRaster({ doc, width: 512, height: 1024, raster, current: () => current,
      render: () => { throw new Error('unexpected fallback'); } });
    check(boundaryObserved, '8 MiB boundary not exercised');
    check(yielded && actual === null, 'foreground interruption or released tile ignored'); doc.released();
    release(source, raster);
  });
  for (const failure of [false, true]) await test(failure ? 'render-error-cleanup' : 'fallback-interrupt', async () => {
    const doc = trackedDocument(); let current = true, chunks = 0, caught = false;
    try {
      const actual = await thumbnailRaster({ doc, width: 913, height: 707, current: () => current, render: async (_, tile) => {
        chunks++; current = false;
        if (failure) throw new Error('synthetic render failure');
        return { width: tile.width, height: tile.height, rgba: new Uint8Array(tile.width * tile.height * 4) };
      } });
      check(actual === null, 'cancelled result returned');
    } catch (error) { check(failure && error.message === 'synthetic render failure', error.message); caught = true; }
    check(caught === failure && chunks === 1, 'unexpected failure or work after interruption'); doc.released();
  });
  return { browser: navigator.userAgent, results, images };
}
