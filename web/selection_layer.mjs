/** Native selection owns hit testing/copy. A single SVG fill owns its appearance:
 * overlapping glyph quads never compound the highlight's opacity. */
const geometry = new WeakMap();
export function registerSelectionGeometry(layer, characters) { geometry.set(layer, { characters, overlay: null }); }
export function selectionPath(quads) {
  return quads.map(points => {
    const area = points.reduce((sum, [x, y], i) => { const next = points[(i + 1) % points.length]; return sum + x * next[1] - y * next[0]; }, 0);
    const ordered = area < 0 ? [...points].reverse() : points;
    return `M${ordered.map(([x, y]) => `${x},${y}`).join('L')}Z`;
  }).join('');
}
export function paintSelection(selection, layers, doc = document) {
  const range = selection && !selection.isCollapsed && selection.rangeCount ? selection.getRangeAt(0) : null;
  const containing = node => (node?.nodeType === 3 ? node.parentElement : node)?.closest?.('.text-layer');
  const first = range && containing(range.startContainer), last = range && containing(range.endContainer);
  const start = layers.indexOf(first), end = layers.indexOf(last);
  for (const [index, layer] of layers.entries()) {
    const record = geometry.get(layer); if (!record) continue;
    if (!range || start < 0 || end < start || index < start || index > end) { record.overlay?.remove(); record.overlay = null; continue; }
    const selected = [];
    for (const { span, quads } of record.characters) {
      // Synthetic reading-order separators have no painted geometry.
      if (quads.length && range.intersectsNode(span)) {
        if ((range.endContainer === span || range.endContainer === span.firstChild) && range.endOffset === 0) continue;
        if (range.startContainer === span.firstChild && range.startOffset === span.textContent.length) continue;
        if (range.startContainer === span && range.startOffset === span.childNodes.length) continue;
        selected.push(...quads);
      }
    }
    if (!selected.length) { record.overlay?.remove(); record.overlay = null; continue; }
    if (!record.overlay) {
      const svg = doc.createElementNS('http://www.w3.org/2000/svg', 'svg');
      svg.setAttribute('class', 'selection-highlights'); svg.setAttribute('aria-hidden', 'true');
      svg.append(doc.createElementNS('http://www.w3.org/2000/svg', 'path'));
      layer.parentElement.append(svg); record.overlay = svg;
    }
    record.overlay.firstChild.setAttribute('d', selectionPath(selected));
  }
}
