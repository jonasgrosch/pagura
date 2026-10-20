/** Match the existing PDF baseline; retain the original painted text on focus. */
export function applyTextStyle(doc, node, widget, style) {
  if (!style) return;
  const name = style.font.replace(/^\//, '').replace(/^[A-Z]{6}\+/, '');
  const family = /Helvetica|Arial|NimbusSans/i.test(name) ? 'Helvetica, Arial, sans-serif'
    : /Times|NimbusRoman/i.test(name) ? 'Times, serif' : /Courier|NimbusMono/i.test(name) ? 'Courier, monospace' : `${JSON.stringify(name)}, sans-serif`;
  const weight = /bold/i.test(name) ? '700' : '400', slant = /italic|oblique/i.test(name) ? 'italic' : 'normal';
  const size = style.size, height = widget.rect[3] - widget.rect[1];
  Object.assign(node.style, { fontFamily: family, fontSize: `${size}px`, fontWeight: weight, fontStyle: slant,
    lineHeight: `${size * 1.2}px`, textAlign: ['left', 'center', 'right'][style.align], paddingLeft: `${Math.max(0, style.align ? 1 : style.left - 1)}px`, paddingRight: '1px' });
  const context = doc.createElement('canvas').getContext('2d');
  if (!context) return;
  context.font = `${slant} ${weight} ${size}px ${family}`;
  const metrics = context.measureText('Hg');
  const ascent = metrics.fontBoundingBoxAscent ?? metrics.actualBoundingBoxAscent ?? size * 0.8;
  const descent = metrics.fontBoundingBoxDescent ?? metrics.actualBoundingBoxDescent ?? size * 0.2;
  if (node.tagName === 'TEXTAREA') {
    node.style.paddingTop = `${Math.max(0, style.baseline - 1 - ascent - (size * 1.2 - ascent - descent) / 2)}px`;
    node.style.paddingBottom = '0px';
  } else {
    const difference = 2 * style.baseline - height - ascent + descent;
    node.style.paddingTop = `${Math.max(0, difference)}px`;
    node.style.paddingBottom = `${Math.max(0, -difference)}px`;
  }
}
