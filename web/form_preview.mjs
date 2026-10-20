import { pagePoint, quadBounds } from './page_geometry.mjs';

/** A disposable draft snapshot supplies the same artwork as the downloaded PDF.
 * Never changes the source, downloads, or accumulates incremental revisions. */
export class FormPreview {
  constructor(editor) { this.editor = editor; this.version = 0; this.snapshot = null; this.timer = null; this.running = false; }
  clear() {
    ++this.version; clearTimeout(this.timer); this.timer = null;
    const snapshot = this.snapshot; this.snapshot = null;
    if (snapshot) void snapshot.client.close(snapshot.handle).catch(() => {});
    for (const record of this.editor.pages.values()) this.clearPage(record);
  }
  clearPage(record) {
    record.previewKey = null;
    for (const canvas of record.previewCanvases ?? []) canvas.remove();
    record.previewCanvases = [];
    for (const control of record.controls) control.node.classList.toggle('form-previewed', false);
  }
  schedule() {
    this.clear();
    if (!this.editor.changes.size || !this.editor.editing) return;
    this.timer = setTimeout(() => { this.timer = null; void this.update(); }, 350);
  }
  async update() {
    // Coalesce typing while a snapshot is being prepared; never pile up saves.
    if (this.running) { this.timer = setTimeout(() => { this.timer = null; void this.update(); }, 350); return; }
    const version = this.version, editing = this.editor.editing;
    if (!editing || !this.editor.changes.size) return;
    this.running = true;
    try {
      const changes = [...this.editor.changes.values()].sort((a, b) => a.index - b.index);
      const bytes = await editing.client.saveForm(editing.handle, changes);
      if (version !== this.version) return;
      const opened = await editing.client.open(bytes, 1);
      if (version !== this.version) { await editing.client.close(opened.document); return; }
      this.snapshot = { client: editing.client, handle: opened.document, version };
      this.editor.redraw();
    } catch (error) {
      if (version === this.version) this.editor.message.textContent = `Draft retained. Preview unavailable: ${error}`;
    } finally { this.running = false; }
  }
  async page(index, record, info, zoom) {
    const snapshot = this.snapshot;
    if (!snapshot) return;
    const ratio = globalThis.devicePixelRatio || 1;
    const key = `${snapshot.version}:${zoom}:${ratio}`;
    if (record.previewKey === key) return;
    this.clearPage(record); record.previewKey = key;
    const scale = zoom * ratio;
    let pixels = 0;
    try {
      const styles = new Map((await snapshot.client.formStyles?.(snapshot.handle, index) ?? []).map(style => [style.index, style]));
      if (snapshot !== this.snapshot || this.editor.pages.get(index) !== record || record.previewKey !== key) return;
      for (const control of record.controls) {
        if (!this.editor.changes.has(control.field) || this.editor.fields?.get(control.field)?.kind === 2) continue;
        const [x0, y0, x1, y1] = control.widget.rect;
        const bounds = quadBounds([pagePoint(info, scale, x0, y0), pagePoint(info, scale, x1, y1)]);
        const x = Math.max(0, Math.floor(bounds.left)), y = Math.max(0, Math.floor(bounds.top));
        const right = Math.min(Math.ceil(info.width * scale - 1e-9), Math.ceil(bounds.left + bounds.width));
        const bottom = Math.min(Math.ceil(info.height * scale - 1e-9), Math.ceil(bounds.top + bounds.height));
        const tile = { x, y, width: right - x, height: bottom - y };
        if (tile.width <= 0 || tile.height <= 0) continue;
        pixels += tile.width * tile.height;
        if (pixels > 4 * 1024 * 1024) throw new Error('Visible form preview exceeds the pixel budget; reduce zoom');
        const image = await snapshot.client.renderTile(snapshot.handle, index, scale, tile);
        if (snapshot !== this.snapshot || this.editor.pages.get(index) !== record || record.previewKey !== key) return;
        const canvas = this.editor.doc.createElement('canvas');
        canvas.className = 'form-preview'; canvas.width = image.width; canvas.height = image.height;
        canvas.setAttribute('aria-hidden', 'true');
        Object.assign(canvas.style, { left: `${x / ratio}px`, top: `${y / ratio}px`, width: `${image.width / ratio}px`, height: `${image.height / ratio}px` });
        const context = canvas.getContext('2d'); if (!context) throw new Error('Canvas 2D rendering is unavailable');
        context.putImageData(new ImageData(new Uint8ClampedArray(image.rgba), image.width, image.height), 0, 0);
        record.layer.append(canvas); record.previewCanvases.push(canvas);
        control.applyStyle?.(styles.get(control.widget.index));
        control.node.classList.toggle('form-previewed', true);
      }
    } catch (error) {
      if (snapshot === this.snapshot) this.editor.message.textContent = `Draft retained. Preview unavailable: ${error}`;
    }
  }
}
