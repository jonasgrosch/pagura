/** Use a finer raster for reduced pages so device-snapped PDF strokes do not
 * dominate the artwork. Native HiDPI is preserved; extra sampling is capped at
 * 4 pixels per CSS pixel. Tile and retained-byte budgets remain unchanged. */
export function renderDensity(zoom, deviceRatio = globalThis.devicePixelRatio || 1) {
  return Math.max(deviceRatio, Math.min(4, 2 / zoom));
}
