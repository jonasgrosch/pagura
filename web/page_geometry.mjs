/** Default user space to CSS pixels, including crop, Rotate and UserUnit (8.3.2). */
export function pagePoint(info, zoom, x, y) {
  const [x0, y0, x1, y1] = info.crop, scale = info.userUnit * zoom;
  const coordinates = info.rotation === 90 ? [y - y0, x - x0]
    : info.rotation === 180 ? [x1 - x, y - y0]
      : info.rotation === 270 ? [y1 - y, x1 - x] : [x - x0, y1 - y];
  return coordinates.map((n) => n * scale);
}
export function pageQuad(info, zoom, quad) {
  return Array.from({ length: 4 }, (_, i) => pagePoint(info, zoom, quad[i * 2], quad[i * 2 + 1]));
}
export function quadBounds(points) {
  let left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity;
  for (const [x, y] of points) { left = Math.min(left, x); top = Math.min(top, y); right = Math.max(right, x); bottom = Math.max(bottom, y); }
  return { left, top, width: right - left, height: bottom - top };
}
/** Convex hit region also accepts source QuadPoints in Z order. */
export function placeQuad(element, points) {
  const bounds = quadBounds(points);
  const center = points.reduce(([x, y], p) => [x + p[0] / 4, y + p[1] / 4], [0, 0]);
  const perimeter = [...points].sort((a, b) => Math.atan2(a[1] - center[1], a[0] - center[0]) - Math.atan2(b[1] - center[1], b[0] - center[0]));
  Object.assign(element.style, { left: `${bounds.left}px`, top: `${bounds.top}px`, width: `${bounds.width}px`, height: `${bounds.height}px`,
    clipPath: `polygon(${perimeter.map(([x, y]) => `${100 * (x - bounds.left) / (bounds.width || 1)}% ${100 * (y - bounds.top) / (bounds.height || 1)}%`).join(',')})` });
  return bounds;
}
/** Inverse page transform, used to retain nullable destination coordinates. */
export function pageUserPoint(info, zoom, x, y) {
  const [x0,y0,x1,y1] = info.crop, scale = info.userUnit * zoom;
  x /= scale; y /= scale;
  return info.rotation === 90 ? [x0+y,y0+x] : info.rotation === 180 ? [x1-x,y0+y]
    : info.rotation === 270 ? [x1-y,y1-x] : [x0+x,y1-y];
}
