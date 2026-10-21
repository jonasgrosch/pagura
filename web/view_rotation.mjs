/** Additional viewing rotation; PDF page geometry and saved bytes stay unchanged. */
export function rotatedSize(width, height, rotation) {
  return rotation % 180 ? { width: height, height: width } : { width, height };
}
export function rotatePoint(x, y, width, height, rotation) {
  return rotation === 90 ? [height - y, x] : rotation === 180 ? [width - x, height - y]
    : rotation === 270 ? [y, width - x] : [x, y];
}
export function unrotatePoint(x, y, width, height, rotation) {
  return rotation === 90 ? [y, height - x] : rotation === 180 ? [width - x, height - y]
    : rotation === 270 ? [width - y, x] : [x, y];
}
export function rotationTransform(width, height, rotation) {
  return rotation === 90 ? `translate(${height}px, 0) rotate(90deg)`
    : rotation === 180 ? `translate(${width}px, ${height}px) rotate(180deg)`
      : rotation === 270 ? `translate(0, ${width}px) rotate(270deg)` : '';
}
/** Viewport intersection expressed in the unrotated raster's coordinate space. */
export function rasterBounds(bounds, viewport, width, height, rotation) {
  const left = Math.max(0, viewport.left - bounds.left), top = Math.max(0, viewport.top - bounds.top);
  const size = rotatedSize(width, height, rotation);
  const right = Math.min(size.width, viewport.right - bounds.left), bottom = Math.min(size.height, viewport.bottom - bounds.top);
  if (right <= left || bottom <= top) return null;
  const points = [[left, top], [right, top], [right, bottom], [left, bottom]]
    .map(([x, y]) => unrotatePoint(x, y, width, height, rotation));
  return { left: Math.min(...points.map(p => p[0])), top: Math.min(...points.map(p => p[1])),
    right: Math.max(...points.map(p => p[0])), bottom: Math.max(...points.map(p => p[1])) };
}
