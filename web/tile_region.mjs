import { rasterBounds } from './view_rotation.mjs';
export const TILE = 256;
/** Same grid for visible and speculative tiles; CSS rotation never changes the
 * underlying raster key. A bounded strip follows either scrolling axis. */
export function tilesInRegion(info, zoom, ratio, rotation, bounds, region, limit = 128) {
  const scale = zoom * ratio;
  const width = Math.max(1, Math.ceil(info.width * scale - 1e-9)), height = Math.max(1, Math.ceil(info.height * scale - 1e-9));
  const area = rasterBounds(bounds, region, info.width * zoom, info.height * zoom, rotation);
  if (!area) return [];
  const left = Math.max(0, Math.floor(area.left * ratio / TILE) * TILE), top = Math.max(0, Math.floor(area.top * ratio / TILE) * TILE);
  const right = Math.min(width, Math.ceil(area.right * ratio)), bottom = Math.min(height, Math.ceil(area.bottom * ratio));
  const requests = [];
  for (let y = top; y < bottom; y += TILE) for (let x = left; x < right; x += TILE) {
    if (requests.length >= limit) throw new Error('Visible page exceeds the tile budget; use a smaller window');
    requests.push({ x, y, width: Math.min(TILE, width - x), height: Math.min(TILE, height - y) });
  }
  return requests;
}
export function aheadRegion(viewport, direction) {
  const vertical = direction.axis === 'y', extent = Math.min(512, (vertical ? viewport.bottom - viewport.top : viewport.right - viewport.left) / 2);
  if (vertical) return { left: viewport.left, right: viewport.right, top: direction.sign > 0 ? viewport.bottom : viewport.top - extent, bottom: direction.sign > 0 ? viewport.bottom + extent : viewport.top };
  return { top: viewport.top, bottom: viewport.bottom, left: direction.sign > 0 ? viewport.right : viewport.left - extent, right: direction.sign > 0 ? viewport.right + extent : viewport.left };
}
