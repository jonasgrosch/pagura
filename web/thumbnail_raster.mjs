import { TILE } from './tile_region.mjs';

const WIDTH = 280, HEIGHT = 332, SUPERSAMPLE = 4;
const MAX_SCRATCH_PIXELS = 2 * 1024 * 1024, MAX_SCRATCH_DIMENSION = 4096;
function canvasFor(doc, width, height) {
  const canvas = doc.createElement('canvas');
  canvas.width = width; canvas.height = height; canvas.setAttribute('aria-hidden', 'true');
  return canvas;
}
function smooth(canvas) {
  const context = canvas.getContext('2d');
  context.imageSmoothingEnabled = true; context.imageSmoothingQuality = 'high';
  return context;
}
function downsample(doc, scratch, width, height) {
  const canvas = canvasFor(doc, width, height);
  smooth(canvas).drawImage(scratch, 0, 0, width, height);
  return canvas;
}
/** Retain 2x thumbnails. Reuse complete page tiles, otherwise supersample in
 * small, interruptible idle chunks. Scratch pixels never enter the cache. */
export async function thumbnailRaster({ doc, width, height, raster, render, current }) {
  const scale = Math.min(WIDTH / width, HEIGHT / height);
  const targetWidth = Math.max(1, Math.ceil(width * scale - 1e-9));
  const targetHeight = Math.max(1, Math.ceil(height * scale - 1e-9));
  if (!current()) return null;
  if (raster && raster.width >= targetWidth && raster.height >= targetHeight
      && Number.isSafeInteger(raster.width) && Number.isSafeInteger(raster.height)
      && raster.width <= MAX_SCRATCH_DIMENSION && raster.height <= MAX_SCRATCH_DIMENSION
      && raster.width * raster.height <= MAX_SCRATCH_PIXELS) {
    const tiles = [];
    for (let y = 0; y < raster.height; y += TILE) for (let x = 0; x < raster.width; x += TILE) {
      const canvas = raster.tiles.get(`${x},${y}`);
      if (canvas?.width === Math.min(TILE, raster.width - x) && canvas.height === Math.min(TILE, raster.height - y)) tiles.push({ canvas, x, y, width: canvas.width, height: canvas.height });
    }
    if (tiles.length === Math.ceil(raster.width / TILE) * Math.ceil(raster.height / TILE)) {
      // Scaling separate tiles exposes their fractional edges and filters each
      // edge without its neighbours. Stitch at integer source pixels first.
      // This scratch canvas is capped at 8 MiB RGBA and never enters the cache.
      const scratch = canvasFor(doc, raster.width, raster.height), context = scratch.getContext('2d');
      try {
        let copied = 0;
        for (const tile of tiles) {
          if (!current() || tile.canvas.width !== tile.width || tile.canvas.height !== tile.height) return null;
          context.drawImage(tile.canvas, tile.x, tile.y);
          // Let foreground input interrupt even a fully cached thumbnail.
          if (++copied % 4 === 0) await new Promise(resolve => setTimeout(resolve, 0));
        }
        return current() ? downsample(doc, scratch, targetWidth, targetHeight) : null;
      } finally { scratch.width = 0; scratch.height = 0; }
    }
  }
  // At most 1120x1328 RGBA scratch pixels (5.68 MiB), plus one <=256px tile.
  const scratch = canvasFor(doc, Math.max(1, Math.ceil(width * scale * SUPERSAMPLE - 1e-9)), Math.max(1, Math.ceil(height * scale * SUPERSAMPLE - 1e-9)));
  const context = scratch.getContext('2d');
  try {
    for (let y = 0; y < scratch.height; y += TILE) for (let x = 0; x < scratch.width; x += TILE) {
      if (!current()) return null;
      const tile = { x, y, width: Math.min(TILE, scratch.width - x), height: Math.min(TILE, scratch.height - y) };
      const image = await render(scale * SUPERSAMPLE, tile);
      if (!current()) return null;
      context.putImageData(new ImageData(new Uint8ClampedArray(image.rgba), image.width, image.height), x, y);
    }
    return downsample(doc, scratch, targetWidth, targetHeight);
  } finally { scratch.width = 0; scratch.height = 0; }
}
