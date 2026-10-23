/** Bounded per-document GPU resources, paired with the host's prepared LRU. */
export const MAX_ENTRIES = 3;
export const MAX_GPU_BYTES = 256 * 1024 * 1024;

export class RetainedSession {
  constructor(engine, gpu, operation = 8) {
    Object.assign(this, {engine, gpu, operation, jobs: new Map(), bytes: 0, evictions: 0, epoch: engine.generation, page: null});
  }

  async clear() {
    for (const entry of this.jobs.values()) await entry.job.destroy();
    this.jobs.clear();this.bytes = 0;
  }

  async synchronize(options) {
    if (this.epoch !== this.engine.generation || this.page !== (options.page ?? 0)) {
      await this.clear();this.epoch = this.engine.generation;this.page = options.page ?? 0;
    }
  }

  key(options) {
    return JSON.stringify([options.page ?? 0, options.dpi ?? 150, options.band ?? 32, ...(options.region ?? [0,0,0,0])]);
  }

  async prepare(options) {
    await this.synchronize(options);
    const key = this.key(options), previous = this.jobs.get(key);
    if (previous) {this.jobs.delete(key);this.jobs.set(key, previous);return {...previous, cache_hit: true};}
    const start = performance.now(), packet = await this.engine.render(this.operation, options);
    if (packet.meta.fallback || !this.gpu) return {fallback: packet.meta.fallback ?? 'Hardware GPU unavailable', packet: packet.meta, setup_frame_ms: performance.now() - start};
    const residentBytes = packet.data.length + packet.meta.width * packet.meta.height * 4 * (this.gpu.readbackCopies ?? 2) + 64;
    if (residentBytes > MAX_GPU_BYTES) throw new Error('GPU resource exceeds retained budget');
    while (this.jobs.size >= MAX_ENTRIES || this.bytes + residentBytes > MAX_GPU_BYTES) {
      const [oldKey, old] = this.jobs.entries().next().value;
      await old.job.destroy();this.jobs.delete(oldKey);this.bytes -= old.bytes;this.evictions++;
    }
    const job = await this.gpu.prepare(packet.data);
    try {
      const upload = await job.retain();
      const entry = {job, bytes: job.resident_bytes, packet: packet.meta, packet_bytes: packet.data.length,
        geometry: job.geometry, setup_frame_ms: performance.now() - start, setup_ms: job.setup_ms, upload_ms: upload.upload_ms};
      this.jobs.set(key, entry);this.bytes += entry.bytes;
      return {...entry, cache_hit: false};
    } catch (error) {await job.destroy();throw error;}
  }

  async render(options) {
    const entry = await this.prepare(options);
    if (entry.fallback) return entry;
    const output = await entry.job.render(null);
    return {...output, entry, cache_hit: entry.cache_hit};
  }

  metadata() {
    return {entries: this.jobs.size, resident_bytes: this.bytes, evictions: this.evictions,
      resources: Array.from(this.jobs.values(), e => ({bytes:e.bytes, packet_bytes:e.packet_bytes, ...e.job.counters()}))};
  }
}
