import { paintSelection } from './selection_layer.mjs';
import { textLayer, selectedText } from './text_layer.mjs';
import { pageQuad, placeQuad } from './page_geometry.mjs';
import { targetLabel, activateTarget } from './link_actions.mjs';
import { OutlineTree } from './outline_tree.mjs';

/** Reading state follows the document epoch, independently of tile generations. */
export class ReadingView {
  constructor({ state, navigate, destination, report, notice, controls }) {
    Object.assign(this, { state, navigate, destination, report, notice, controls });
    this.$ = (id) => document.getElementById(id); this.pages = new Map(); this.labels = []; this.labelIndex = new Map();
    this.matches = []; this.current = -1; this.searchId = 0; this.searchNote = '';
    this.tree = new OutlineTree(this.$('outline-tree'), (target) => this.activate(target));
    this.$('find-open').addEventListener('click', () => this.showSearch());
    this.$('find-close').addEventListener('click', () => { this.$('find-panel').hidden = true; this.$('find-open').setAttribute('aria-expanded', 'false'); this.cancelSearch(); this.$('find-open').focus(); });
    for (const id of ['find-query', 'find-case', 'find-word']) this.$(id).addEventListener(id === 'find-query' ? 'input' : 'change', () => { void this.search(); });
    this.$('find-next').addEventListener('click', () => { void this.move(1); }); this.$('find-previous').addEventListener('click', () => { void this.move(-1); });
    this.$('find-query').addEventListener('keydown', (event) => {
      if (event.key === 'Enter') { event.preventDefault(); void this.move(event.shiftKey ? -1 : 1); }
      if (event.key === 'Escape') { event.preventDefault(); this.$('find-close').click(); }
    });
    this.$('copy-page').addEventListener('click', () => { void this.copyPage(); });
    window.addEventListener('keydown', (event) => { if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') { event.preventDefault(); this.showSearch(); } });
    document.addEventListener('selectionchange', () => {
      if (this.selectionScheduled) return;
      this.selectionScheduled = true;
      requestAnimationFrame(() => { this.selectionScheduled = false;
        paintSelection(window.getSelection(), [...this.pages.entries()].sort(([a], [b]) => a - b).map(([, record]) => record.textLayer).filter(Boolean));
      });
    });
    document.addEventListener('copy', (event) => {
      const text = selectedText(window.getSelection());
      if (text !== null && event.clipboardData) { event.preventDefault(); event.clipboardData.setData('text/plain', text); }
    });
  }
  reset() {
    this.cancelSearch(); this.pages.clear(); this.labels = []; this.labelIndex.clear(); this.tree.set([]); this.matches = []; this.current = -1;
    this.$('page-label').textContent = ''; this.$('reading-status').textContent = ''; this.$('copy-page').disabled = true;
  }
  cancelSearch() { this.searchNote = ''; this.pendingScroll = null; this.searchId++; this.state().client?.invalidateSearch(); this.matches = []; this.current = -1; this.highlight(); this.count(); }
  async open() {
    const { client, handle, epoch } = this.state(); this.reset();
    // Independent navigation failures do not hide available page text or pixels.
    const results = await Promise.allSettled([client.labels(handle), client.outlines(handle)]);
    if (epoch !== this.state().epoch) return;
    if (results[0].status === 'fulfilled') {
      this.labels = results[0].value;
      this.labels.forEach((label, i) => { if (!this.labelIndex.has(label)) this.labelIndex.set(label, i); });
      for (const record of this.pages.values()) this.nameLinks(record);
    } else this.$('reading-status').textContent = String(results[0].reason);
    if (results[1].status === 'fulfilled') {
      try { this.tree.set(results[1].value); } catch (error) { this.$('reading-status').textContent = String(error); }
    }
    else this.$('reading-status').textContent += ` ${results[1].reason}`;
    this.controls();
    if (this.$('find-query').value && !this.$('find-panel').hidden) void this.search();
  }
  label(page) { return this.labels[page] ?? String(page + 1); }
  nameLinks(record) {
    for (const { area, target } of record.links ?? []) {
      const name = targetLabel(target, (page) => this.label(page));
      area.setAttribute('aria-label', name); area.setAttribute('title', name);
    }
  }
  pageIndex(value) {
    // Explicit displayed index wins for the default numeric labels. Exact PDF
    // labels, including nonnumeric and duplicated labels, resolve to the first.
    if (this.labelIndex.has(value)) return this.labelIndex.get(value);
    const index = Number(value) - 1;
    return Number.isInteger(index) ? index : -1;
  }
  prune(indexes) { for (const key of this.pages.keys()) if (!indexes.includes(key)) this.pages.delete(key); }
  async page(index, info, element) {
    if (this.pages.get(index)?.element === element) return;
    const { client, handle, epoch, zoom } = this.state(); const record = { info, element, zoom, text: null };
    this.pages.set(index, record);
    const results = await Promise.allSettled([client.pageText(handle, index), client.links(handle, index)]);
    if (epoch !== this.state().epoch || this.pages.get(index) !== record) return;
    try {
      if (results[0].status === 'fulfilled') {
        const data = results[0].value;
        const total = [...this.pages.values()].reduce((n, p) => n + (p.characterCount ?? 0), data.characters.length);
        const quads = data.characters.reduce((n, c) => n + c.quadCount, 0);
        if (total > 100000 || quads > 100000) throw new Error('Visible text exceeds the DOM geometry budget');
        const layer = textLayer(data, info, zoom);
        record.textLayer = layer; record.text = { text: data.text }; record.characterCount = data.characters.length;
        element.append(layer); this.$('copy-page').disabled = false;
      } else this.$('reading-status').textContent = `Page ${index + 1} text: ${results[0].reason}`;
      if (results[1].status === 'fulfilled') {
        const areas = results[1].value.reduce((n, link) => n + link.quads.length, 0);
        const total = [...this.pages.values()].reduce((n, p) => n + (p.linkAreas ?? 0), areas);
        if (total > 10000) throw new Error('Visible links exceed the 10000-area DOM budget');
        record.linkAreas = areas; record.links = [];
        const layer = document.createElement('div'); layer.className = 'link-layer';
        for (const link of results[1].value) {
          const actions = link.target;
          // A focusable link has no href, so the browser cannot bypass policy
          // with middle click, drag, a context menu or an unconfirmed activation.
          const node = document.createElement('a'); node.setAttribute('role', 'link'); node.setAttribute('tabindex', '0'); node.className = 'pdf-link';
          node.addEventListener('focus', () => { this.notice(node.getAttribute('aria-label')); });
          // Each quad has its own exact hit area; only the first participates in
          // the focus order so a multiline link is announced once.
          for (const [i, quad] of link.quads.entries()) {
            const area = i === 0 ? node : document.createElement('a'); area.className = 'pdf-link';
            record.links.push({ area, target: actions });
            area.addEventListener('mouseenter', () => { this.notice(area.getAttribute('aria-label')); });
            if (i) { area.setAttribute('tabindex', '-1'); area.setAttribute('aria-hidden', 'true'); }
            placeQuad(area, pageQuad(info, zoom, quad));
            area.addEventListener('click', (event) => { event.preventDefault(); void this.activate(actions); });
            area.addEventListener('keydown', (event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); void this.activate(actions); } });
            layer.append(area);
          }
        }
        this.nameLinks(record); element.append(layer);
      } else this.$('reading-status').textContent += ` Page ${index + 1} links: ${results[1].reason}`;
      this.highlight(); this.scrollCurrent();
    } catch (error) { this.$('reading-status').textContent = String(error); }
  }
  activate(target) {
    return activateTarget(target, { destination: this.destination, report: this.report, notice: this.notice,
      named: (name) => { const s = this.state(); return this.navigate(({ NextPage: s.page + 1, PrevPage: s.page - 1, FirstPage: 0, LastPage: s.count - 1 })[name]); } });
  }
  showSearch() { this.$('find-panel').hidden = false; this.$('find-open').setAttribute('aria-expanded', 'true'); this.$('find-query').focus(); }
  count(suffix) { if (suffix !== undefined) this.searchNote = suffix; this.$('find-count').textContent = `${this.matches.length ? this.current + 1 : 0} / ${this.matches.length}${this.searchNote}`; }
  async search() {
    this.cancelSearch(); const id = this.searchId, s = this.state(), query = this.$('find-query').value;
    if (!query || s.handle === null) return;
    const token = s.client.searchEpoch, flags = (this.$('find-case').checked ? 1 : 0) | (this.$('find-word').checked ? 2 : 0) | 4;
    let page = 0, offset = 0, quads = 0, skipped = 0;
    this.count(' — searching…');
    while (page < s.count) {
      // One page at a time allows scrolling and a newer query between requests.
      let response;
      try { response = await s.client.search(s.handle, query, page, page + 1, offset, 128, flags, token); }
      catch (error) {
        if (id !== this.searchId || s.epoch !== this.state().epoch) return;
        this.$('reading-status').textContent = `Search page ${page + 1}: ${error}`; skipped++; page++; offset = 0; continue;
      }
      if (id !== this.searchId || s.epoch !== this.state().epoch || response.cancelled) return;
      if (response.nextPage < page || response.nextPage > page + 1 || response.done !== (response.nextPage === page + 1)
          || response.matches.some((match) => match.page !== page)) { this.report(new Error('Invalid search response cursor')); return; }
      for (const match of response.matches) {
        quads += match.quads.length;
        if (this.matches.length >= 1000 || quads > 10000) { this.count(' — result budget reached'); this.highlight(); return; }
        this.matches.push(match);
      }
      if (this.current < 0 && this.matches.length) { this.current = 0; void this.move(0); }
      this.count(' — searching…'); this.highlight();
      if (response.nextPage === page && response.nextOffset <= offset) { this.report(new Error('Search cursor did not advance')); return; }
      page = response.nextPage; offset = response.nextOffset;
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    this.count(skipped ? ` — ${skipped} pages unavailable` : ''); this.highlight();
  }
  async move(delta) {
    if (!this.matches.length) return;
    this.current = (Math.max(0, this.current) + delta + this.matches.length) % this.matches.length;
    const id = this.searchId, match = this.matches[this.current]; this.count(); this.highlight();
    await this.navigate(match.page);
    if (id !== this.searchId || this.matches[this.current] !== match) return;
    this.highlight();
    this.pendingScroll = match; this.scrollCurrent();
  }
  scrollCurrent() {
    const match = this.pendingScroll;
    if (!match || this.matches[this.current] !== match) return;
    const record = this.pages.get(match.page);
    if (record && match.quads.length) {
      const points = pageQuad(record.info, this.state().zoom, match.quads[0]);
      this.pendingScroll = null;
      this.state().scrollMatch(match.page, Math.min(...points.map((p) => p[1])), points);
    }
  }

  highlight() {
    for (const [page, record] of this.pages) {
      record.highlights?.remove();
      const layer = document.createElement('div'); layer.className = 'search-highlights'; layer.setAttribute('aria-hidden', 'true');
      this.matches.forEach((match, index) => {
        if (match.page !== page) return;
        for (const quad of match.quads) {
          const mark = document.createElement('div'); mark.className = index === this.current ? 'search-match current-match' : 'search-match';
          placeQuad(mark, pageQuad(record.info, record.zoom, quad)); layer.append(mark);
        }
      });
      record.highlights = layer; record.element.append(layer);
    }
  }
  async copyPage() {
    const s = this.state(), epoch = s.epoch;
    try {
      const text = this.pages.get(s.page)?.text ?? await s.client.pageText(s.handle, s.page);
      if (epoch !== this.state().epoch) return;
      await navigator.clipboard.writeText(text.text); this.$('reading-status').textContent = `Page ${s.page + 1} text copied`;
    } catch (error) { this.report(error); }
  }
}
