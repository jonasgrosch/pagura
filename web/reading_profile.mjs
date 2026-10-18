// @ts-check
import { textLayer } from './text_layer.mjs';
/**
 * @import { PageTransform } from './page_geometry.mjs'
 * @import { PageText } from './reading_protocol.mjs'
 */
/** Two warmups and seven construction samples. Pass the real browser document
 * to measure DOM creation; excludes layout, Canvas painting and extraction.
 * @param {PageText} data
 * @param {PageTransform} info
 */
export function profileTextLayer(data, info, doc = document) {
  const samples = [];
  for (let i = -2; i < 7; i++) {
    const start = performance.now(),
      layer = textLayer(data, info, 1, doc);
    const elapsed = performance.now() - start;
    if (layer.children.length !== data.characters.length) throw new Error('Incomplete text layer');
    if (i >= 0) samples.push(elapsed);
  }
  return samples;
}
