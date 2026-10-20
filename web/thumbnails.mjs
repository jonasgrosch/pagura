import { rotatedSize, rotationTransform } from './view_rotation.mjs';

const ROW = 202, CACHE_SIZE = 32, DENSITY = 2;
/** Virtualized sidebar: bounded pixels/DOM, one idle request at a time, no form controls. */
export class Thumbnails {
  constructor({ state, navigate, panel, list }) {
    Object.assign(this, { state, navigate, panel, list });
    this.epoch = null; this.cache = new Map(); this.rows = new Map(); this.running = false; this.revision = 0;
    panel.addEventListener('scroll', () => this.refresh(), { passive: true });
    panel.addEventListener('keydown', event => {
      const { count, page } = this.state();
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key) || !count) return;
      event.preventDefault();
      const index = event.key === 'Home' ? 0 : event.key === 'End' ? count - 1
        : Math.max(0, Math.min(count - 1, page + (event.key === 'ArrowDown' ? 1 : -1)));
      void this.navigate(index); this.select(index, true);
    });
  }
  refreshDrafts() {
    if (this.stopped) return;
    clearTimeout(this.draftFrame);
    this.draftFrame = setTimeout(() => {
      this.draftFrame = null; if (this.stopped) return;
      this.revision++;
      for (const [index, row] of this.rows) this.paint(index, row);
    }, 120);
  }
  release(entry) { for (const canvas of [entry.canvas, entry.composite]) if (canvas) { canvas.remove(); canvas.width = 0; canvas.height = 0; } }
  destroy() { this.stopped = true; clearTimeout(this.timer); clearTimeout(this.draftFrame); for (const entry of this.cache.values()) this.release(entry); this.cache.clear(); }
  reset() {
    clearTimeout(this.timer); this.timer = null; for (const entry of this.cache.values()) this.release(entry); this.cache.clear(); this.rows.clear(); this.list.replaceChildren();
    this.panel.scrollTop = 0; this.epoch = this.state().epoch;
  }
  select(index, reveal = false) {
    if (reveal && (index * ROW < this.panel.scrollTop || (index + 1) * ROW > this.panel.scrollTop + this.panel.clientHeight)) {
      this.panel.scrollTop = Math.max(0, index * ROW - (this.panel.clientHeight - ROW) / 2);
    }
    this.refresh();
  }
  refresh() {
    const state = { ...this.state() };
    if (this.epoch !== state.epoch) this.reset();
    this.list.style.height = `${state.count * ROW}px`;
    if (!state.visible) return;
    const first = Math.max(0, Math.floor(this.panel.scrollTop / ROW) - 1);
    const end = Math.min(state.count, first + Math.ceil(this.panel.clientHeight / ROW) + 3);
    for (const [index, row] of this.rows) if (index < first || index >= end) { row.button.remove(); this.rows.delete(index); }
    for (let index = first; index < end; index++) {
      let row = this.rows.get(index);
      if (!row) {
        const button = document.createElement('button'); button.className = 'thumbnail'; button.style.top = `${index * ROW}px`;
        const picture = document.createElement('span'); picture.className = 'thumbnail-picture';
        const number = document.createElement('span'); number.className = 'thumbnail-number';
        button.append(picture, number); button.addEventListener('click', () => { void this.navigate(index); });
        row = { button, picture, number }; this.rows.set(index, row); this.list.append(button);
      }
      row.button.setAttribute('aria-label', `Go to page ${index + 1}`);
      if (index === state.page) row.button.setAttribute('aria-current', 'page'); else row.button.removeAttribute('aria-current');
      row.number.textContent = state.label(index);
      this.paint(index, row);
    }
    this.schedule();
  }
  paint(index, row) {
    const entry = this.cache.get(index), rotation = this.state().rotation(index);
    if (!entry) {
      if (!row.picture.firstChild) { const placeholder = document.createElement('span'); placeholder.className = 'thumbnail-placeholder'; placeholder.textContent = '…'; row.picture.append(placeholder); }
      return;
    }
    if (entry.error) { row.picture.textContent = 'Preview unavailable'; return; }
    const { width, height } = entry;
    const state = this.state(), record = state.forms?.(index);
    const pending = state.dirtyForms && !record;
    row.button.title = pending ? 'Saved page preview — form changes will appear when this page is viewed' : `Page ${index + 1}`;
    row.number.textContent = state.label(index) + (pending ? ' •' : '');
    if (entry.revision !== this.revision || entry.recordKey?.deref() !== record?.layer) {
      entry.revision = this.revision; entry.recordKey = record?.layer ? new WeakRef(record.layer) : null;
      const overlays = record?.info && !record.layer.hidden ? [...record.buttonCanvases ?? [], ...record.previewCanvases ?? []].filter(canvas => !canvas.hidden) : [];
      if (overlays.length) {
        entry.composite ??= document.createElement('canvas'); entry.composite.setAttribute('aria-hidden', 'true');
        entry.composite.width = entry.canvas.width; entry.composite.height = entry.canvas.height;
        const context = entry.composite.getContext('2d'); context.drawImage(entry.canvas, 0, 0);
        const sx = entry.canvas.width / (record.info.width * record.zoom), sy = entry.canvas.height / (record.info.height * record.zoom);
        for (const canvas of overlays) context.drawImage(canvas, parseFloat(canvas.style.left) * sx, parseFloat(canvas.style.top) * sy,
          parseFloat(canvas.style.width) * sx, parseFloat(canvas.style.height) * sy);
      } else { if (entry.composite) { entry.composite.width = 0; entry.composite.height = 0; } entry.composite = null; }
    }
    const canvas = entry.composite ?? entry.canvas;
    const size = rotatedSize(width, height, rotation), scale = Math.min(140 / size.width, 166 / size.height);
    const wrapper = document.createElement('span'); Object.assign(wrapper.style, { position: 'relative', width: `${size.width * scale}px`, height: `${size.height * scale}px` });
    Object.assign(canvas.style, { position: 'absolute', left: '0', top: '0', width: `${width * scale}px`, height: `${height * scale}px`, transformOrigin: '0 0', transform: rotationTransform(width * scale, height * scale, rotation) });
    // Avoid moving a canvas on every main-page scroll update.
    if (row.canvas === canvas && row.rotation === rotation) return;
    wrapper.append(canvas); row.picture.replaceChildren(wrapper); row.canvas = canvas; row.rotation = rotation;
  }
  schedule() {
    if (this.stopped || this.timer || this.running || !this.state().visible || !this.state().idle) return;
    this.timer = setTimeout(() => { this.timer = null; void this.renderNext(); }, 100);
  }
  async renderNext() {
    if (this.stopped) return;
    const state = { ...this.state() };
    if (!state.visible || state.handle === null) return;
    const index = [...this.rows.keys()].find(index => !this.cache.has(index));
    if (index === undefined) return;
    if (!state.idle || (state.client.queued ?? 0) > 0) { this.schedule(); return; }
    this.running = true;
    try {
      const info = await state.client.pageInfo(state.handle, index);
      if (this.stopped || state.epoch !== this.state().epoch || !this.state().visible) return;
      let width = (info.crop[2] - info.crop[0]) * info.userUnit, height = (info.crop[3] - info.crop[1]) * info.userUnit;
      if (info.rotation % 180) [width, height] = [height, width];
      const scale = Math.min(140 * DENSITY / width, 166 * DENSITY / height);
      // Yield if foreground rendering started while metadata was loading.
      if (!this.state().idle) return;
      const image = await state.client.renderTile(state.handle, index, scale, { x: 0, y: 0, width: Math.max(1, Math.ceil(width * scale - 1e-9)), height: Math.max(1, Math.ceil(height * scale - 1e-9)) });
      if (this.stopped || state.epoch !== this.state().epoch) return;
      const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height; canvas.setAttribute('aria-hidden', 'true');
      canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(image.rgba), image.width, image.height), 0, 0);
      this.cache.set(index, { canvas, width, height });
    } catch { if (!this.stopped && state.epoch === this.state().epoch) this.cache.set(index, { error: true }); }
    finally {
      this.running = false;
      while (this.cache.size > CACHE_SIZE) {
        const old = [...this.cache.keys()].find(key => !this.rows.has(key));
        if (old === undefined) break;
        this.release(this.cache.get(old)); this.cache.delete(old);
      }
      if (!this.stopped && state.epoch === this.state().epoch) { const row = this.rows.get(index); if (row) this.paint(index, row); }
      this.schedule();
    }
  }
}
