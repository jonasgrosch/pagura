import { pagePoint, quadBounds } from './page_geometry.mjs';
/** Table 149 destination views. Content-box fits require engine content bounds;
 * reporting that gap is preferable to silently substituting the page box. */
export function destinationView(action, info, width, height, currentZoom, retained = {}) {
  const [a, b, c, d] = action.coordinates, [x0, y0, x1, y1] = info.crop;
  if (action.fit >= 5) throw new Error('FitB/FitBH/FitBV require page content bounds, which the engine does not expose');
  let zoom = currentZoom, x = null, y = null;
  const coordinate = (left, top) => {
    left ??= retained.left ?? null; top ??= retained.top ?? null;
    const point = pagePoint(info, zoom, left ?? x0, top ?? y1), rotated = info.rotation === 90 || info.rotation === 270;
    if (rotated ? top !== null : left !== null) x = point[0];
    if (rotated ? left !== null : top !== null) y = point[1];
  };
  if (action.fit === 0) { if (c !== null) zoom = c; coordinate(a, b); }
  if (action.fit === 1) { zoom = Math.min(width / info.width, height / info.height); x = 0; y = 0; }
  if (action.fit === 2) { zoom = width / info.width; coordinate(null, a); }
  if (action.fit === 3) { zoom = height / info.height; coordinate(a, null); }
  if (action.fit === 4) {
    const bounds = quadBounds([[a,b],[c,b],[c,d],[a,d]].map(([x,y]) => pagePoint(info, 1, x, y)));
    if (!bounds.width || !bounds.height) throw new Error('Empty FitR destination');
    zoom = Math.min(width / bounds.width, height / bounds.height); x = bounds.left * zoom; y = bounds.top * zoom;
  }
  if (!Number.isFinite(zoom) || zoom < 0.1 || zoom > 8) throw new Error('Destination zoom exceeds the viewer range (10%–800%)');
  return { zoom, x, y };
}
