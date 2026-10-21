import { TILE } from './tile_region.mjs';

const WIDTH = 280, HEIGHT = 332, SUPERSAMPLE = 4;
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
/** Retain 2x thumbnails. Reuse complete page tiles, otherwise supersample in
 * small, interruptible idle chunks. Scratch pixels never enter the cache. */
export async function thumbnailRaster({ doc, width, height, raster, render, current }) {
  const scale = Math.min(WIDTH / width, HEIGHT / height);
  const targetWidth = Math.max(1, Math.ceil(width * scale - 1e-9));
  const targetHeight = Math.max(1, Math.ceil(height * scale - 1e-9));
  if (!current()) return null;
  if (raster && raster.width >= targetWidth && raster.height >= targetHeight
      && Math.ceil(raster.width / TILE) * Math.ceil(raster.height / TILE) <= 128) {
    const tiles = [];
    for (let y = 0; y < raster.height; y += TILE) for (let x = 0; x < raster.width; x += TILE) {
      const canvas = raster.tiles.get(`${x},${y}`);
      if (canvas?.width === Math.min(TILE, raster.width - x) && canvas.height === Math.min(TILE, raster.height - y)) tiles.push({ canvas, x, y });
    }
    if (tiles.length === Math.ceil(raster.width / TILE) * Math.ceil(raster.height / TILE)) {
      const canvas = canvasFor(doc, targetWidth, targetHeight), context = smooth(canvas);
      for (const tile of tiles) context.drawImage(tile.canvas, tile.x * targetWidth / raster.width, tile.y * targetHeight / raster.height,
        tile.canvas.width * targetWidth / raster.width, tile.canvas.height * targetHeight / raster.height);
      return canvas;
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
    const canvas = canvasFor(doc, targetWidth, targetHeight);
    smooth(canvas).drawImage(scratch, 0, 0, targetWidth, targetHeight);
    return canvas;
  } finally { scratch.width = 0; scratch.height = 0; }
}
