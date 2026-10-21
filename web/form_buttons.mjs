import { renderDensity } from './render_density.mjs';
import { pagePoint, quadBounds } from './page_geometry.mjs';

/** Prewarm source artwork once per visible page/zoom; a click only flips canvases. */
export class ButtonArtwork {
  constructor(editor) { this.editor = editor; }
  async page(index, record, info, zoom) {
    const ratio = renderDensity(zoom), key = `${zoom}:${ratio}`;
    if (record.buttonKey === key) return;
    record.buttonKey = key;
    for (const canvas of record.buttonCanvases ?? []) canvas.remove();
    record.buttonCanvases = [];
    const controls = record.controls.filter(control => this.editor.fields.get(control.field)?.kind === 2);
    if (!controls.length) return;
    const fields = [...new Set(controls.map(control => control.field))].map(index => this.editor.fields.get(index));
    const editing = this.editor.editing;
    const current = () => editing === this.editor.editing && this.editor.pages.get(index) === record && record.buttonKey === key;
    for (const control of controls) { control.artwork = new Map(); control.node.disabled = true; }
    let pixels = 0;
    try {
      const variants = 1 + Math.max(...fields.map(field => field.options.length));
      if (variants > 64) throw new Error('Button artwork exceeds the state budget');
      for (let variant = 0; variant < variants; variant++) {
        const changes = fields.map(field => {
          const control = controls.find(control => control.field === field.index);
          const requireSelection = Boolean(control.widget.flags & 1 && control.widget.flags & 2);
          if (!variant && !requireSelection) return { index: field.index, kind: 2 };
          const option = field.options[Math.max(0, variant - 1) % field.options.length];
          return { index: field.index, kind: 3, widget: option.key, state: option.state };
        }).sort((a, b) => a.index - b.index);
        const bytes = await editing.client.saveForm(editing.handle, changes);
        if (!current()) return;
        const opened = await editing.client.open(bytes, 1);
        try {
          if (!current()) return;
          for (const control of controls) {
            const selected = control.selected(changes.find(change => change.index === control.field));
            if (control.artwork.has(selected)) continue;
            const [x0,y0,x1,y1] = control.widget.rect, scale = zoom * ratio;
            const bounds = quadBounds([pagePoint(info, scale, x0, y0), pagePoint(info, scale, x1, y1)]);
            const x = Math.max(0, Math.floor(bounds.left)), y = Math.max(0, Math.floor(bounds.top));
            const right = Math.min(Math.ceil(info.width * scale - 1e-9), Math.ceil(bounds.left + bounds.width));
            const bottom = Math.min(Math.ceil(info.height * scale - 1e-9), Math.ceil(bounds.top + bounds.height));
            const tile = { x, y, width: right - x, height: bottom - y };
            if (tile.width <= 0 || tile.height <= 0) continue;
            pixels += tile.width * tile.height;
            if (pixels > 4 * 1024 * 1024) throw new Error('Button artwork exceeds the pixel budget; reduce zoom');
            const image = await editing.client.renderTile(opened.document, index, scale, tile);
            if (!current()) return;
            const canvas = this.editor.doc.createElement('canvas'); canvas.className = 'form-preview';
            canvas.width = image.width; canvas.height = image.height; canvas.setAttribute('aria-hidden', 'true');
            Object.assign(canvas.style, { left: `${x / ratio}px`, top: `${y / ratio}px`, width: `${image.width / ratio}px`, height: `${image.height / ratio}px` });
            const context = canvas.getContext('2d'); if (!context) throw new Error('Canvas 2D unavailable');
            context.putImageData(new ImageData(new Uint8ClampedArray(image.rgba), image.width, image.height), 0, 0);
            canvas.hidden = true; record.layer.append(canvas); record.buttonCanvases.push(canvas); control.artwork.set(selected, canvas);
          }
        } finally { await editing.client.close(opened.document); }
      }
      if (current()) {
        for (const control of controls) control.node.disabled = false;
        this.editor.sync();
      }
    } catch (error) { if (current()) this.editor.message.textContent = `Button artwork unavailable: ${error}`; }
  }
}
