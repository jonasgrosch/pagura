export function formsAllowPreload(active, fields) { return !active || fields?.size === 0; }
export const TILE_CACHE_BYTES = 64 * 1024 * 1024;
/** One owner for full-resolution canvases, attached or detached. Visible tiles
 * are pinned; eviction releases the backing store as well as the reference. */
export class TileCache {
  constructor({ limit = TILE_CACHE_BYTES, entries = 512 } = {}) {
    this.limit = limit; this.maxEntries = entries; this.items = new Map(); this.pinned = new Set(); this.bytes = 0;
    this.hits = 0; this.misses = 0; this.evictions = 0;
  }
  static key(page, scale, tile) { return `${page}:${scale}:${tile.x},${tile.y},${tile.width},${tile.height}`; }
  has(key) { return this.items.has(key); }
  get(key) {
    const entry = this.items.get(key);
    if (!entry) { this.misses++; return null; }
    this.hits++; this.items.delete(key); this.items.set(key, entry); return entry.canvas;
  }
  pin(keys) { this.pinned = new Set(keys); }
  put(key, canvas) {
    if (this.items.has(key)) return false;
    const bytes = canvas.width * canvas.height * 4;
    if (!Number.isSafeInteger(bytes) || bytes <= 0 || bytes > this.limit) return false;
    while (this.bytes + bytes > this.limit || this.items.size >= this.maxEntries) {
      const old = [...this.items.keys()].find(key => !this.pinned.has(key));
      if (old === undefined) return false;
      this.delete(old); this.evictions++;
    }
    this.items.set(key, { canvas, bytes }); this.bytes += bytes; return true;
  }
  delete(key) {
    const entry = this.items.get(key); if (!entry) return;
    entry.canvas.remove(); entry.canvas.width = 0; entry.canvas.height = 0;
    this.bytes -= entry.bytes; this.items.delete(key);
  }
  clear() { for (const key of this.items.keys()) this.delete(key); this.pinned.clear(); }
  stats() { return { bytes: this.bytes, limit: this.limit, entries: this.items.size, hits: this.hits, misses: this.misses, evictions: this.evictions }; }
}

/** Bounded speculative work. Every dispatch rechecks the foreground gate; a
 * generation change drops the plan. The host cannot interrupt a running tile. */
export class TilePreloader {
  constructor({ allowed, idle = () => true, updated = () => {}, render, now = () => performance.now(), delay = 500 }) {
    Object.assign(this, { allowed, idle, updated, render, now, delay });
    this.version = 0; this.jobs = []; this.running = false; this.timer = null; this.lastActivity = now();
    this.completed = 0; this.discarded = 0; this.stopped = false;
  }
  cancel() { this.version++; this.jobs = []; this.lastActivity = this.now(); clearTimeout(this.timer); this.timer = null; }
  plan(jobs) { this.jobs = jobs.slice(0, 16); this.schedule(); }
  schedule() {
    if (this.stopped || this.running || this.timer || !this.jobs.length || !this.allowed()) return;
    this.timer = setTimeout(() => { this.timer = null; void this.step(); }, Math.max(32, this.delay - (this.now() - this.lastActivity)));
  }
  async step() {
    if (this.stopped || this.running || !this.jobs.length || !this.allowed()) return;
    if (!this.idle()) { this.schedule(); return; }
    if (this.now() - this.lastActivity < this.delay) { this.schedule(); return; }
    const version = this.version, job = this.jobs.shift(); this.running = true;
    const current = () => !this.stopped && version === this.version && this.allowed();
    try { if (await this.render(job, current)) this.completed++; else this.discarded++; }
    catch { this.jobs = []; } // Speculative failures must not turn a readable page into an error.
    finally { this.running = false; this.updated(); this.schedule(); }
  }
  destroy() { this.stopped = true; this.cancel(); }
}
