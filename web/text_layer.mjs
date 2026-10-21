import { registerSelectionGeometry } from './selection_layer.mjs';
import { pageQuad, quadBounds } from './page_geometry.mjs';
const encoder = new TextEncoder(), decoder = new TextDecoder('utf-8', { fatal: true });
export const MAX_TEXT_CHARACTERS = 50000;
/** Transparent DOM text in extracted reading order. No PDF string becomes HTML. */
export function textLayer(data, info, zoom, doc = document) {
  if (data.characters.reduce((n,c) => n + c.quadCount, 0) > 100000) throw new Error('Page text exceeds the 100000-quad DOM budget');
  if (data.characters.length > MAX_TEXT_CHARACTERS) throw new Error('Page text exceeds the 50000-character DOM budget');
  const layer = doc.createElement('div'); layer.className = 'text-layer'; layer.setAttribute('role', 'document'); layer.setAttribute('aria-label', 'Selectable page text');
  const bytes = encoder.encode(data.text); let previous = [0, 0];
  const context = doc.createElement('canvas').getContext('2d'); context.font = '1px monospace';
  const widths = new Map(), selection = [];
  for (const character of data.characters) {
    const span = doc.createElement('span'); span.textContent = decoder.decode(bytes.subarray(character.start, character.end));
    const width = widths.get(span.textContent) ?? context.measureText(span.textContent).width;
    if (widths.size < 4096) widths.set(span.textContent, width);
    span.style.letterSpacing = `${1 - width}px`;
    span.setAttribute('data-start', String(character.start)); span.setAttribute('data-end', String(character.end));
    const quads = data.quads.slice(character.quadStart, character.quadStart + character.quadCount).map(quad => pageQuad(info, zoom, quad));
    selection.push({ span, quads });
    if (character.quadCount) {
      let points = pageQuad(info, zoom, data.quads[character.quadStart]);
      if (character.quadCount > 1) {
        let left=Infinity, top=Infinity, right=-Infinity, bottom=-Infinity;
        for (let i=character.quadStart;i<character.quadStart+character.quadCount;i++) {
          const b=quadBounds(pageQuad(info,zoom,data.quads[i])); left=Math.min(left,b.left); top=Math.min(top,b.top); right=Math.max(right,b.left+b.width); bottom=Math.max(bottom,b.top+b.height);
        }
        points = [[left,bottom],[right,bottom],[right,top],[left,top]];
      }
      const [bottom, , right, top] = points;
      // A one-em transparent cell maps affinely to the metric quad, including
      // rotated/vertical text. Glyph ink is painted only by the renderer.
      const a = right[0] - top[0], b = right[1] - top[1], c = bottom[0] - top[0], d = bottom[1] - top[1];
      Object.assign(span.style, { transform: `matrix(${a},${b},${c},${d},${top[0]},${top[1]})` });
      previous = points[1];
    } else Object.assign(span.style, { transform: `translate(${previous[0]}px,${previous[1]}px)` });
    layer.append(span);
  }
  registerSelectionGeometry(layer, selection);
  return layer;
}
/** Extract only text nodes from a native range: CSS line wrapping never inserts
 * guessed line breaks into copy text. Separators already come from extraction. */
export function selectedText(selection) {
  if (!selection || selection.isCollapsed || !selection.rangeCount) return null;
  const range = selection.getRangeAt(0);
  const layer = (node) => (node.nodeType === 3 ? node.parentElement : node)?.closest?.('.text-layer');
  const first = layer(range.startContainer), last = layer(range.endContainer);
  if (!first || !last) return null;
  if (first === last) return range.cloneContents().textContent;
  // Across pages, clone only each page's text layer, excluding controls,
  // messages and link labels from the clipboard.
  const layers = [...document.querySelectorAll('.text-layer')];
  const start = layers.indexOf(first), end = layers.indexOf(last);
  if (start < 0 || end < start) return null;
  return layers.slice(start, end + 1).map((element, i, values) => {
    const part = document.createRange(); part.selectNodeContents(element);
    if (i === 0) part.setStart(range.startContainer, range.startOffset);
    if (i === values.length - 1) part.setEnd(range.endContainer, range.endOffset);
    return part.cloneContents().textContent;
  }).join('\n\n');
}
