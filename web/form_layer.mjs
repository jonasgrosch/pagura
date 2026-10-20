import { pagePoint } from './page_geometry.mjs';

/** Position controls in PDF user space; the same transform handles crop/rotation/zoom. */
export function positionWidget(node, rect, info, zoom) {
  const [x0, y0, x1, y1] = rect;
  const origin = pagePoint(info, zoom, x0, y1), x = pagePoint(info, zoom, x0 + 1, y1), y = pagePoint(info, zoom, x0, y1 - 1);
  Object.assign(node.style, { width: `${x1 - x0}px`, height: `${y1 - y0}px`,
    transform: `matrix(${x[0] - origin[0]},${x[1] - origin[1]},${y[0] - origin[0]},${y[1] - origin[1]},${origin[0]},${origin[1]})` });
}
const sameName = (a, b) => a?.length === b?.length && a?.every((value, index) => value === b[index]);
export function widgetControl(doc, field, widget, changed) {
  if (field.kind === 3 || field.flags & 1) return null;
  const options = field.options.filter((option) => option.key === widget.index);
  // A button with ambiguous/missing on states cannot safely act like a checkbox.
  if (field.kind === 2 && options.length !== 1) return null;
  const button = field.kind === 2, radio = button && Boolean(widget.flags & 1);
  let node;
  if (button) { node = doc.createElement('input'); node.type = radio ? 'radio' : 'checkbox'; }
  else if (field.kind === 1 && !(field.flags & 8)) {
    node = doc.createElement('select'); node.multiple = Boolean(field.flags & 2);
    if (widget.flags & 8) node.size = Math.max(2, Math.min(10, field.options.length));
    if (!node.multiple) { const empty = doc.createElement('option'); empty.value = ''; empty.textContent = ''; node.append(empty); }
    for (const option of field.options) { const item = doc.createElement('option'); item.value = String(option.key); item.textContent = option.label; node.append(item); }
  } else { node = doc.createElement(field.flags & 4 ? 'textarea' : 'input'); if (!(field.flags & 4)) node.type = 'text'; }
  node.className = 'form-widget'; node.setAttribute('aria-label', field.name || `Field ${field.index + 1}`);
  if (radio) node.name = `form-field-${field.index}`;
  if (radio) node.setAttribute('aria-label', `${field.name}: ${options[0].label}`);
  node.setAttribute('autocomplete', 'off'); node.setAttribute('data-widget', String(widget.index));
  node.style.fontSize = `${Math.min(12, Math.max(6, (widget.rect[3] - widget.rect[1]) * 0.6))}px`;
  const sync = (draft) => {
    node.classList.toggle('form-dirty', Boolean(draft));
    if (button) {
      node.checked = Boolean(draft ? draft.kind === 3 && (draft.widget === widget.index || (!radio || widget.flags & 4) && sameName(draft.state, options[0].state)) : options[0].selected);
    } else if (node.tagName === 'SELECT') {
      const indices = draft?.indices ?? field.options.filter((o) => o.selected).map((o) => o.key);
      for (const option of node.options) option.selected = option.value === '' ? !indices.length : indices.includes(Number(option.value));
    } else {
      const value = draft?.value ?? field.value;
      if (node.value !== value) node.value = value;
    }
  };
  node.addEventListener('input', () => {
    if (button) changed(node.checked ? { index: field.index, kind: 3, widget: widget.index, state: options[0].state } : { index: field.index, kind: 2 });
    else if (node.tagName === 'SELECT') changed({ index: field.index, kind: 1, indices: Array.from(node.selectedOptions).filter((o) => o.value !== '').map((o) => Number(o.value)) });
    else changed({ index: field.index, kind: field.kind === 0 ? 0 : 4, value: node.value });
  });
  sync(); return { node, field: field.index, widget, sync };
}
