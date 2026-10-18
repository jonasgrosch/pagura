import { pagePoint, quadBounds } from './page_geometry.mjs';
/** ISO 32000-2 12.3.2.2, Table 149 destination views. Content-box fits require engine content bounds;
 * reporting that gap is preferable to silently substituting the page box. */
export function destinationView(action, info, width, height, currentZoom, retained = {}) {
  if (action.fit >= 5) return contentDestinationView(action, info, width, height, currentZoom, retained);
  const [a, b, c, d] = action.coordinates, [x0, , , y1] = info.crop;
  let requestedZoom = currentZoom, bounds;
  if (action.fit === 0 && c !== null && c !== 0) requestedZoom = c;
  if (action.fit === 1) requestedZoom = Math.min(width / info.width, height / info.height);
  if (action.fit === 2) requestedZoom = width / info.width;
  if (action.fit === 3) requestedZoom = height / info.height;
  if (action.fit === 4) {
    bounds = quadBounds([[a,b],[c,b],[c,d],[a,d]].map(([x,y]) => pagePoint(info, 1, x, y)));
    if (!bounds.width || !bounds.height) throw new Error('Empty FitR destination');
    requestedZoom = Math.min(width / bounds.width, height / bounds.height);
  }
  if (!Number.isFinite(requestedZoom)) throw new Error('Invalid destination zoom');
  const zoom = Math.max(0.1, Math.min(8, requestedZoom));
  let x = null, y = null;
  const coordinate = (left, top) => {
    left ??= retained.left ?? null; top ??= retained.top ?? null;
    const point = pagePoint(info, zoom, left ?? x0, top ?? y1), rotated = info.rotation === 90 || info.rotation === 270;
    if (rotated ? top !== null : left !== null) x = point[0];
    if (rotated ? left !== null : top !== null) y = point[1];
  };
  if (action.fit === 0) coordinate(a, b);
  if (action.fit === 1) { x = 0; y = 0; }
  if (action.fit === 2) coordinate(null, a);
  if (action.fit === 3) coordinate(a, null);
  if (bounds) { x = bounds.left * zoom; y = bounds.top * zoom; }
  const plan = { zoom, x, y };
  if (zoom !== requestedZoom) plan.notice = `Destination zoom limited to ${Math.round(zoom * 100)}% (viewer range: 10%–800%).`;
  return plan;
}

/** Content-fit integration point. DisplayList::content_bounds() must supply
 * default-user-space painted bounds through an additive host request before
 * these views can be calculated; a crop-box substitution changes their meaning. */
function contentDestinationView(_action, _info, _width, _height, _currentZoom, _retained) {
  throw new Error('FitB/FitBH/FitBV require page content bounds, which the engine does not expose');
}
