import { pagePoint, quadBounds } from './page_geometry.mjs';
/** ISO 32000-2 12.3.2.2, Table 149 destination views. */
export function destinationView(action, info, width, height, currentZoom, retained = {}, contentBounds = null) {
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
  if (action.fit >= 5) {
    const [left,bottom,right,top] = contentBounds ?? info.crop;
    bounds = quadBounds([[left,bottom],[right,bottom],[right,top],[left,top]].map(([x,y]) => pagePoint(info,1,x,y)));
    // A degenerate conservative rectangle cannot define a fit scale.
    const w = bounds.width || info.width, h = bounds.height || info.height;
    requestedZoom = action.fit === 5 ? Math.min(width/w,height/h) : action.fit === 6 ? width/w : height/h;
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
  if (bounds) {
    x = bounds.left * zoom; y = bounds.top * zoom;
    if (action.fit === 5) { x -= (width - bounds.width * zoom)/2; y -= (height - bounds.height * zoom)/2; }
    if (action.fit === 6 || action.fit === 7) {
      const horizontal = action.fit === 6, rotated = info.rotation === 90 || info.rotation === 270;
      const value = a ?? (horizontal ? retained.top : retained.left) ?? null;
      const point = pagePoint(info, zoom, horizontal ? x0 : value ?? x0, horizontal ? value ?? y1 : y1);
      if (horizontal !== rotated) y = value === null ? null : point[1];
      else x = value === null ? null : point[0];
    }
  }
  const plan = { zoom, x, y };
  if (zoom !== requestedZoom) plan.notice = `Destination zoom limited to ${Math.round(zoom * 100)}% (viewer range: 10%–800%).`;
  return plan;
}
