/** Page heights in user units, independent of device pixels and DOM state. */
export class PageLayout {
  constructor(count) {
    if (!Number.isInteger(count) || count < 0 || count > 200000) throw new Error('Page layout budget exceeded');
    this.count = count;
    this.gap = 24;
    this.heights = new Float64Array(count).fill(792);
    this.tree = new Float64Array(count + 1);
    for (let i = 1; i <= count; i++) this.tree[i] = (i & -i) * 792;
  }
  setHeight(page, height) {
    if (!Number.isInteger(page) || page < 0 || page >= this.count || !Number.isFinite(height) || height <= 0)
      throw new Error('Invalid page geometry');
    const delta = height - this.heights[page];
    this.heights[page] = height;
    for (let i = page + 1; i <= this.count; i += i & -i) this.tree[i] += delta;
  }
  top(page, zoom) {
    let height = 0;
    for (let i = page; i > 0; i -= i & -i) height += this.tree[i];
    return height * zoom + page * this.gap;
  }
  height(zoom) {
    return Math.max(0, this.top(this.count, zoom) - this.gap);
  }
  /** Page with the greatest visible height; equal coverage retains the current
   * page. The surface has 24px padding above the first page. Binary search skips
   * offscreen pages; the loop is bounded by the viewport's visible page count. */
  dominantPage(scrollTop, viewportHeight, zoom, current = 0) {
    const top = scrollTop - this.gap,
      bottom = top + viewportHeight;
    let best = Math.min(current, Math.max(0, this.count - 1)),
      coverage = 0;
    let index = this.pageAt(top, zoom),
      start = this.top(index, zoom);
    for (; index < this.count && start < bottom; index++) {
      const height = this.heights[index] * zoom;
      const visible = Math.max(0, Math.min(bottom, start + height) - Math.max(top, start));
      if (visible > coverage || (visible === coverage && index === current)) {
        best = index;
        coverage = visible;
      }
      start += height + this.gap;
    }
    return best;
  }
  pageAt(offset, zoom) {
    let low = 0,
      high = this.count;
    while (low < high) {
      const middle = Math.floor((low + high) / 2);
      if (this.top(middle + 1, zoom) <= offset) low = middle + 1;
      else high = middle;
    }
    return Math.min(low, Math.max(0, this.count - 1));
  }
}
