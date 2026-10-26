import { renderDensity } from './render_density.mjs';
import { TileCache, TilePreloader, formsAllowPreload } from './tile_cache.mjs';
import { tilesInRegion, aheadRegion } from './tile_region.mjs';
import { Thumbnails } from './thumbnails.mjs';
import { rotatedSize, rotatePoint, rotationTransform } from './view_rotation.mjs';
import { FormEditor } from './form_editor.mjs';
import { pageUserPoint } from './page_geometry.mjs';
import { DocumentServices } from './document_services.mjs';
import { ReadingView } from './reading_view.mjs';
import { destinationView } from './destinations.mjs';
import { SessionClient } from './client.mjs';
import { MAX_FILE, HostError } from './protocol.mjs';
import { loadFixture } from './fixtures.mjs';
import { PageLayout } from './layout.mjs';
import { requestPassword, cancelPassword } from './password.mjs';

const $ = (id) => document.getElementById(id);
const viewport = $('viewport'), surface = $('page-surface'), status = $('status'), errorBox = $('error');
const pageInput = $('page'), zoomInput = $('zoom');
// Start with the page unobscured on compact screens; the sidebar remains available.
if (window.matchMedia?.('(max-width: 640px)').matches) {
  $('document-sidebar').hidden = true;
  $('sidebar-toggle').setAttribute('aria-expanded', 'false');
}
const MAX_TILES = 128, MAX_PAGES = 32, MAX_EXTENT = 10000000;
let client, handle = null, layout = new PageLayout(0), page = 0, zoom = 1;
let documentEpoch = 0, generation = 0, drawing = false, scheduled = false, dirty = false;
const infos = new Map(), views = new Map(), rotations = new Map();
const tileCache = new TileCache();
let direction = { axis: 'y', sign: 1 }, lastScroll = [0, 0], pointerDown = false, lastInteraction = 0;
function interactionQuiet() {
  return !pointerDown && !document.hidden && performance.now() - lastInteraction >= 500
    && (document.activeElement?.id === 'file' || !['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName));
}
const debug = new URL(window.location.href).searchParams.get('debug') === '1'
  && ['localhost', '127.0.0.1', '[::1]'].includes(new URL(window.location.href).hostname);
let source = null, recoveryAttempts = 0, recovering = false;

const services = new DocumentServices({ state: () => ({ client, handle, epoch: documentEpoch }) });
const reader = new ReadingView({
  state: () => ({ client, handle, epoch: documentEpoch, page, count: layout.count, zoom,
    scrollMatch: (index, y, points) => { const info = infos.get(index);
      if (info && points) y = Math.min(...points.map(([x, y]) => rotatePoint(x, y, info.width * zoom, info.height * zoom, rotations.get(index) ?? 0)[1]));
      viewport.scrollTop = layout.top(index, zoom) + Math.max(0, y - 40); invalidate(); } }),
  navigate, destination: followDestination, report: (error) => { $('reading-status').textContent = String(error); }, controls,
  notice: readingNotice,
});
const editor = new FormEditor({
  state: () => ({ client, handle, epoch: documentEpoch, recovery: source?.recovery, bytes: source?.bytes, password: source?.password }),
  adopt: adoptSaved, download: downloadPdf, redraw: invalidate, presentationChanged: () => thumbnails?.refreshDrafts(),
});
const preloader = new TilePreloader({
  allowed: () => handle !== null && !drawing && !dirty && interactionQuiet() && !editor.busy
    && !editor.preview?.running && !editor.preview?.timer && formsAllowPreload(editor.active, editor.fields),
  idle: () => (client?.queued ?? 0) === 0,
  render: renderAhead, updated: () => { updateCacheStats(); thumbnails.schedule(); },
});
const thumbnails = new Thumbnails({
  state: () => ({ client, handle, epoch: documentEpoch, count: layout.count, page,
    idle: !drawing && !dirty && !editor.busy && !editor.preview?.running && !editor.preview?.timer
      && interactionQuiet() && !preloader.running && !(preloader.allowed() && preloader.jobs.length), visible: !$('document-sidebar').hidden && !$('thumbnails-panel').hidden,
    label: index => reader.label(index), rotation: index => rotations.get(index) ?? 0,
    forms: index => editor.pages.get(index), dirtyForms: editor.changes.size > 0,
    raster: index => {
      const info = infos.get(index), view = views.get(index), scale = zoom * renderDensity(zoom, window.devicePixelRatio);
      return info?.width && view && !view.failed ? { tiles: view.tiles,
        width: Math.max(1, Math.ceil(info.width * scale - 1e-9)), height: Math.max(1, Math.ceil(info.height * scale - 1e-9)) } : null;
    } }),
  navigate, panel: $('thumbnails-panel'), list: $('thumbnail-list'),
});
function displaySize(info, index = page) { return rotatedSize(info.width, info.height, rotations.get(index) ?? 0); }
function displayInfo(info, index = page) { return { ...info, ...displaySize(info, index), rotation: (info.rotation + (rotations.get(index) ?? 0)) % 360 }; }
// Fillable fields are interactive by default; strict rendering is an opt-in diagnostic mode.
function downloadPdf() {
  if (!source) return;
  const url = URL.createObjectURL(new Blob([source.bytes], { type: 'application/pdf' }));
  const link = document.createElement('a'); link.href = url;
  link.download = ($('filename').textContent.replace(/\.pdf$/i, '') || 'document') + '-filled.pdf';
  document.body.append(link); link.click(); link.remove(); setTimeout(() => URL.revokeObjectURL(url), 1000);
}
async function adoptSaved(bytes, epoch) {
  if (epoch !== documentEpoch) return false;
  const current = client, previous = handle, position = anchor();
  const opened = await current.open(bytes, source.recovery);
  if (epoch !== documentEpoch) { await current.close(opened.document); return false; }
  ++documentEpoch; tileCache.clear(); preloader.cancel(); handle = opened.document;
  errorBox.textContent = ''; errorBox.hidden = true;
  source.password.fill(0); source = { bytes, password: new Uint8Array(), recovery: source.recovery };
  reader.reset(); services.reset(); infos.clear(); views.clear(); surface.replaceChildren();
  layout = new PageLayout(opened.pages); page = Math.min(position.index, Math.max(0, opened.pages - 1));
  await current.close(previous);
  if (current !== client || handle !== opened.document) return false;
  controls(); void reader.open();
  if (opened.pages) { await pageInfo(page, documentEpoch); updateLayout(); restore({ ...position, index: page }); invalidate(); }
  return true;
}
function readingNotice(message) { $('reading-status').textContent = message; }
function report(error) { errorBox.textContent = String(error); errorBox.hidden = false; status.textContent = 'Page unavailable'; }
function controls() {
  const count = layout.count;
  pageInput.disabled = !count; zoomInput.disabled = !count;
  $('previous').disabled = !count || page === 0; $('next').disabled = !count || page === count - 1;
  pageInput.max = String(count);
  if (document.activeElement !== pageInput) pageInput.value = String(page + 1);
  $('count').textContent = `of ${count}`;
  $('page-label').textContent = count ? `Label: ${reader.label(page)}` : '';
  $('copy-page').disabled = !count; $('rotate-view').disabled = !count;
  $('empty-state').hidden = handle !== null || !!source;
  thumbnails.refresh();
  editor.controls(); services.controls();
}
function createClient() {
  if (!crossOriginIsolated || typeof SharedArrayBuffer !== 'function') throw new Error('Cross-origin isolation is required. Start python3 tools/serve_web.py and open http://localhost:8000.');
  const current = new SessionClient(new Worker(new URL('./worker.mjs', import.meta.url), { type: 'module' }), { debug,
    onFailure: (error) => { if (client === current) void recover(error); } });
  return current;
}
async function recover(error) {
  if (recovering) return;
  tileCache.clear(); preloader.cancel(); services.reset();
  if (!source || recoveryAttempts >= 1) {
    handle = null; client?.destroy(); services.controls();
    for (const view of views.values()) view.element.remove(); views.clear();
    report(`${error}. Automatic recovery stopped; reopen the file to retry.`); return;
  }
  recovering = true; recoveryAttempts++; tileCache.clear(); preloader.cancel();
  const saved = source, position = anchor(), epoch = ++documentEpoch;
  handle = null; infos.clear(); views.clear(); surface.replaceChildren();
  report(`${error}. Restarting the PDF host…`); status.textContent = 'Recovering document…';
  try {
    client?.destroy(); client = createClient(); const current = client;
    await current.hello();
    const opened = await current.open(saved.bytes, saved.recovery, saved.password);
    if (epoch !== documentEpoch) return;
    handle = opened.document; page = Math.min(position.index, Math.max(0, opened.pages - 1));
    if (layout.count !== opened.pages) layout = new PageLayout(opened.pages);
    if (opened.pages) { await pageInfo(page, epoch); if (epoch !== documentEpoch) return; }
    updateLayout();
    if (opened.pages) restore({ ...position, index: page });
    controls(); void reader.open(); editor.recovered();
    errorBox.textContent = `${error}. PDF host restarted; document reopened at page ${page + 1}, ${Math.round(zoom * 100)}%.`;
    errorBox.hidden = false; invalidate();
  } catch (failure) {
    if (epoch === documentEpoch) {
      handle = null; client?.destroy(); services.controls();
      report(`${failure}. Automatic recovery stopped; reopen the file to retry.`);
    }
  } finally { recovering = false; }
}
function invalidate() {
  preloader.cancel();
  if (handle !== null) { generation = client.invalidate(handle); status.textContent = 'Updating visible pages…'; }
  dirty = true; schedule();
}
function schedule() {
  if (scheduled) return;
  scheduled = true;
  requestAnimationFrame(() => { scheduled = false; void draw(); });
}
function anchor(point = 0) {
  const index = layout.pageAt(viewport.scrollTop + point, zoom);
  return { index, fraction: (viewport.scrollTop + point - layout.top(index, zoom)) / (layout.heights[index] * zoom || 1), point };
}
function restore(position) {
  viewport.scrollTop = Math.max(0, layout.top(position.index, zoom) + position.fraction * layout.heights[position.index] * zoom - position.point);
}
function updateLayout() {
  const height = layout.height(zoom);
  if (!Number.isFinite(height) || height > MAX_EXTENT) throw new Error('Document exceeds the viewer layout budget; reduce zoom');
  let width = Math.max(1, viewport.clientWidth - 48);
  for (const [index, info] of infos) if (info.width) width = Math.max(width, displaySize(info, index).width * zoom);
  if (width > MAX_EXTENT) throw new Error('Page exceeds the viewer layout budget; reduce zoom');
  surface.style.height = `${height}px`;
  surface.style.width = `${width}px`;
  surface.hidden = !layout.count;
  for (const [index, view] of views) {
    const info = infos.get(index);
    Object.assign(view.element.style, { top: `${layout.top(index, zoom)}px`,
      width: `${(info?.width ? displaySize(info, index).width : 612) * zoom}px`, height: `${layout.heights[index] * zoom}px` });
    if (info?.width) Object.assign(view.content.style, { width: `${info.width * zoom}px`, height: `${info.height * zoom}px`, transform: rotationTransform(info.width * zoom, info.height * zoom, rotations.get(index) ?? 0) });
  }
}
function geometry(info) {
  let width = (info.crop[2] - info.crop[0]) * info.userUnit;
  let height = (info.crop[3] - info.crop[1]) * info.userUnit;
  if (info.rotation === 90 || info.rotation === 270) [width, height] = [height, width];
  return { ...info, width, height };
}
async function pageInfo(index, epoch) {
  const existing = infos.get(index);
  if (existing) return existing.promise ?? existing;
  const promise = client.pageInfo(handle, index).then((raw) => {
    if (epoch !== documentEpoch) return null;
    const position = anchor(), info = geometry(raw);
    const offset = position.fraction * layout.heights[position.index] * zoom;
    infos.set(index, info); layout.setHeight(index, displaySize(info, index).height); updateLayout();
    // Newly measured heights keep the same pixel offset in the anchor page;
    // zoom changes instead retain a proportional position in known geometry.
    position.fraction = Math.min(offset, layout.heights[position.index] * zoom + layout.gap - 1)
      / (layout.heights[position.index] * zoom);
    restore(position);
    // Retain compact heights for every page, but bound detailed metadata.
    if (infos.size > 256) for (const key of infos.keys()) {
      if (!views.has(key) && key !== index) { infos.delete(key); break; }
    }
    return info;
  }).catch((error) => { if (epoch === documentEpoch) infos.delete(index); throw error; });
  infos.set(index, { promise }); return promise;
}
function pageView(index) {
  if (views.has(index)) return views.get(index);
  const element = document.createElement('section'); element.className = 'pdf-page';
  element.setAttribute('data-page', String(index)); element.setAttribute('role', 'region');
  element.setAttribute('aria-label', `Page ${index + 1}`); element.setAttribute('aria-busy', 'true');
  const message = document.createElement('p'); message.className = 'page-message'; message.textContent = `Loading page ${index + 1}…`;
  const content = document.createElement('div'); content.className = 'page-content'; content.append(message); element.append(content);
  let following = null, nextIndex = Infinity;
  for (const [number, view] of views) if (number > index && number < nextIndex) { following = view.element; nextIndex = number; }
  surface.insertBefore(element, following);
  const view = { element, content, message, tiles: new Map(), failed: false }; views.set(index, view); return view;
}
function visiblePages() {
  const top = Math.max(0, viewport.scrollTop - 24), bottom = top + viewport.clientHeight;
  const first = layout.pageAt(top, zoom), last = layout.pageAt(bottom, zoom);
  if (last - first + 1 > MAX_PAGES) throw new Error('Too many visible pages; increase zoom');
  return Array.from({ length: last - first + 1 }, (_, offset) => first + offset);
}
function tileRequests(index, info) {
  return tilesInRegion(info, zoom, renderDensity(zoom, window.devicePixelRatio), rotations.get(index) ?? 0,
    views.get(index).element.getBoundingClientRect(), viewport.getBoundingClientRect(), MAX_TILES);
}
function removeView(index, view) {
  // Detach cached canvases from discarded page trees so they cannot retain form/text DOM.
  for (const canvas of view.tiles.values()) canvas.remove();
  view.tiles.clear(); view.element.remove(); views.delete(index);
}
function tileCanvas(image) {
  const canvas = document.createElement('canvas'); canvas.className = 'tile'; canvas.width = image.width; canvas.height = image.height;
  canvas.setAttribute('aria-hidden', 'true');
  const context = canvas.getContext('2d'); if (!context) throw new Error('Canvas 2D rendering is unavailable');
  context.putImageData(new ImageData(new Uint8ClampedArray(image.rgba.buffer, image.rgba.byteOffset, image.rgba.byteLength), image.width, image.height), 0, 0);
  return canvas;
}
function updateCacheStats() {
  const stats = tileCache.stats();
  $('cache-summary').textContent = `Page tiles: ${(stats.bytes / 1048576).toFixed(1)} MiB / 64 MiB · ${stats.entries} cached · ${stats.hits} reused · ${preloader.completed} preloaded`;
}
function planAhead() {
  if (!preloader.allowed() || !layout.count) return;
  const region = aheadRegion(viewport.getBoundingClientRect(), direction), surfaceBounds = surface.getBoundingClientRect();
  const first = layout.pageAt(viewport.scrollTop + region.top - viewport.getBoundingClientRect().top - 24, zoom);
  const last = layout.pageAt(viewport.scrollTop + region.bottom - viewport.getBoundingClientRect().top - 24, zoom);
  const pages = Array.from({ length: Math.min(3, last - first + 1) }, (_, i) => direction.sign > 0 ? first + i : last - i);
  const jobs = pages.map(index => ({ index, info: infos.get(index)?.width ? infos.get(index) : null, region, surfaceBounds,
    epoch: documentEpoch, token: generation, client, handle, zoom, ratio: renderDensity(zoom, window.devicePixelRatio), rotation: rotations.get(index) ?? 0,
    top: layout.top(index, zoom), direction: { ...direction } }));
  preloader.plan(Array.from({ length: 16 }, (_, i) => jobs[i % jobs.length]));
}
async function renderAhead(job, current) {
  const valid = () => current() && job.epoch === documentEpoch && job.token === generation && job.client === client;
  if (!valid()) return false;
  if (!job.info) job.info = geometry(await job.client.pageInfo(job.handle, job.index));
  if (!valid()) return false;
  const size = rotatedSize(job.info.width, job.info.height, job.rotation), scale = job.zoom * job.ratio;
  const bounds = { left: job.surfaceBounds.left + (parseFloat(surface.style.width) - size.width * job.zoom) / 2,
    top: job.surfaceBounds.top + job.top };
  const tiles = tilesInRegion(job.info, job.zoom, job.ratio, job.rotation, bounds, job.region);
  tiles.sort((a, b) => {
    const coordinate = tile => rotatePoint(tile.x / job.ratio, tile.y / job.ratio, job.info.width * job.zoom, job.info.height * job.zoom, job.rotation)[job.direction.axis === 'y' ? 1 : 0];
    return job.direction.sign * (coordinate(a) - coordinate(b));
  });
  const tile = tiles.find(tile => !tileCache.has(TileCache.key(job.index, scale, tile)));
  if (!tile) return false;
  const image = await job.client.renderGenerationTile(job.handle, job.index, scale, tile, job.token);
  if (!valid() || image.cancelled) return false;
  const canvas = tileCanvas(image);
  if (!tileCache.put(TileCache.key(job.index, scale, tile), canvas)) { canvas.width = 0; canvas.height = 0; return false; }
  return true;
}
function failPage(index, error) {
  const view = views.get(index); if (!view) return;
  view.failed = true;
  for (const key of tileCache.items.keys()) if (key.startsWith(`${index}:`)) tileCache.delete(key);
  view.tiles.clear(); view.content.replaceChildren(view.message);
  view.message.hidden = false; view.message.setAttribute('role', 'alert'); view.message.textContent = String(error);
  view.element.setAttribute('aria-busy', 'false'); report(`Page ${index + 1}: ${error}`);
}
async function diagnostics(epoch) {
  const items = await client.diagnostics(handle);
  if (epoch !== documentEpoch) return;
  const repairs = items.filter((item) => item.kind === 'repair').length;
  $('diagnostic-count').textContent = `Repairs: ${repairs} · Information: ${items.length - repairs}`;
  $('repair-list').replaceChildren(...items.map((item) => {
    const li = document.createElement('li'); li.textContent = `${item.kind === 'repair' ? 'Repair' : 'Information'} [${item.tag}] ${item.message}`; return li;
  }));
  // Keep diagnostics available without expanding implementation details over the document.
}
async function draw() {
  if (drawing || handle === null || !layout.count) return;
  drawing = true; dirty = false;
  const epoch = documentEpoch, token = generation, documentHandle = handle;
  try {
    // Heights become precise only as the visible range is loaded.
    for (let pass = 0; pass < MAX_PAGES; pass++) {
      const indexes = visiblePages();
      reader.prune(indexes); editor.prune(indexes);
      for (const [index, view] of views) if (!indexes.includes(index)) removeView(index, view);
      for (const index of indexes) pageView(index);
      const missing = indexes.find((index) => !infos.get(index)?.width);
      if (missing === undefined) break;
      await pageInfo(missing, epoch);
      if (epoch !== documentEpoch || token !== generation) return;
    }
    updateLayout();
    const requested = new Map(); let total = 0;
    for (const index of visiblePages()) {
      const info = infos.get(index); if (!info?.width || views.get(index)?.failed) continue;
      const tiles = tileRequests(index, info); total += tiles.length; requested.set(index, tiles);
    }
    if (total > MAX_TILES) throw new Error('Visible pages exceed the tile budget; increase zoom');
    tileCache.pin([...requested].flatMap(([index, tiles]) => tiles.map(tile => TileCache.key(index, zoom * renderDensity(zoom, window.devicePixelRatio), tile))));
    for (const index of visiblePages()) {
      if (epoch !== documentEpoch || token !== generation) return;
      const info = infos.get(index), view = pageView(index);
      if (!info?.width || view.failed) continue;
      const requests = requested.get(index) ?? [];
      const keys = new Set(requests.map(({ x, y }) => `${x},${y}`));
      for (const [key, canvas] of view.tiles) if (!keys.has(key)) { canvas.remove(); view.tiles.delete(key); }
      try {
        for (const tile of requests) {
          if (epoch !== documentEpoch || token !== generation) return;
          const key = `${tile.x},${tile.y}`; if (view.tiles.has(key)) continue;
          const cacheKey = TileCache.key(index, zoom * renderDensity(zoom, window.devicePixelRatio), tile);
          let canvas = tileCache.get(cacheKey);
          if (!canvas) {
            status.textContent = 'Rendering visible tiles…';
            const image = await client.renderGenerationTile(documentHandle, index, zoom * renderDensity(zoom, window.devicePixelRatio), tile, token);
            if (epoch !== documentEpoch || token !== generation || image.cancelled) return;
            canvas = tileCanvas(image);
            if (!tileCache.put(cacheKey, canvas)) { canvas.width = 0; canvas.height = 0; throw new Error('Visible tiles exceed the raster cache budget'); }
          }
          const ratio = renderDensity(zoom, window.devicePixelRatio);
          Object.assign(canvas.style, { left: `${tile.x / ratio}px`, top: `${tile.y / ratio}px`, width: `${canvas.width / ratio}px`, height: `${canvas.height / ratio}px` });
          view.tiles.set(key, canvas); view.content.append(canvas);
        }
        view.message.hidden = true; view.element.setAttribute('aria-busy', 'false');
        await reader.page(index, info, view.content);
        if (epoch === documentEpoch && token === generation) await editor.page(index, info, view.content, zoom);
        if (epoch !== documentEpoch || token !== generation) return;
      } catch (error) { if (epoch === documentEpoch && token === generation) failPage(index, error); }
    }
    if (epoch === documentEpoch && token === generation) {
      await diagnostics(epoch);
      if (epoch === documentEpoch && token === generation) {
        controls();
        updateCacheStats();
        status.textContent = [...views.values()].some((view) => view.failed) ? 'Some pages are unavailable' : 'Ready';
      }
    }
  } catch (error) { if (epoch === documentEpoch && token === generation) report(error); }
  finally { drawing = false; if (dirty) schedule(); else scheduleQuietWork(); }
}
function fit(mode, info) {
  info = displayInfo(info);
  const width = (viewport.clientWidth - 48) / info.width;
  return mode === 'fit-page' ? Math.min(width, (viewport.clientHeight - 48) / info.height) : width;
}
function changeZoom(value, point = viewport.clientHeight / 2) {
  if (!Number.isFinite(value) || handle === null) return;
  const position = anchor(point), previousZoom = zoom;
  zoom = Math.min(8, Math.max(0.1, value));
  try { updateLayout(); } catch (error) { zoom = previousZoom; report(error); return; }
  for (const [index, view] of views) removeView(index, view); tileCache.pin([]); reader.prune([]); editor.prune([]);
  $('custom-zoom').textContent = `${Math.round(zoom * 100)}%`; restore(position); invalidate();
}
async function navigate(index) {
  if (!Number.isInteger(index) || index < 0 || index >= layout.count || handle === null) { pageInput.value = String(page + 1); return; }
  const epoch = documentEpoch; status.textContent = 'Loading page…';
  try {
    const info = await pageInfo(index, epoch); if (!info || epoch !== documentEpoch) return;
    page = index;
    if (zoomInput.value.startsWith('fit-')) changeZoom(fit(zoomInput.value, info));
    viewport.scrollTop = layout.top(index, zoom); pageInput.value = String(index + 1); controls(); thumbnails.select(page, true); invalidate();
  } catch (error) { if (epoch === documentEpoch) report(error); }
}
async function followDestination(action) {
  const epoch = documentEpoch, rawPrevious = infos.get(page), previous = rawPrevious?.width ? displayInfo(rawPrevious) : null;
  const retained = previous?.width ? pageUserPoint(previous, zoom,
    viewport.scrollLeft - (parseFloat(surface.style.width) - previous.width * zoom) / 2,
    viewport.scrollTop - layout.top(page, zoom)) : null;
  const info = await pageInfo(action.page, epoch);
  if (!info || epoch !== documentEpoch) return;
  const bounds = action.fit >= 5 ? (await client.contentBounds(handle, action.page)).bounds : null;
  if (epoch !== documentEpoch) return;
  const plan = destinationView(action, displayInfo(info, action.page), viewport.clientWidth - 48, viewport.clientHeight - 48, zoom, retained ? { left: retained[0], top: retained[1] } : {}, bounds);
  zoomInput.value = 'custom'; changeZoom(plan.zoom, 0);
  await navigate(action.page); if (epoch !== documentEpoch) return;
  if (plan.y !== null) viewport.scrollTop = layout.top(action.page, zoom) + plan.y;
  if (plan.x !== null) viewport.scrollLeft = Math.max(0, (parseFloat(surface.style.width) - displaySize(info, action.page).width * zoom) / 2 + plan.x);
  invalidate();
  if (plan.notice) readingNotice(plan.notice);
}
async function openFile(file) {
  if (!file) return;
  cancelPassword();
  source?.password.fill(0); source = null; recoveryAttempts = 0;
  const epoch = ++documentEpoch; preloader.cancel(); tileCache.clear(); reader.reset(); services.reset(); editor.reset(); rotations.clear();
  handle = null; infos.clear(); views.clear(); surface.replaceChildren(); surface.hidden = true;
  layout = new PageLayout(0); page = 0; controls(); errorBox.hidden = true;
  $('filename').textContent = file.name; status.textContent = 'Opening…';
  $('diagnostic-count').textContent = 'Repairs: 0 · Information: 0'; $('repair-list').replaceChildren();
  try {
    if (file.size > MAX_FILE) throw new Error('PDF exceeds the 512 MiB input budget');
    client?.destroy();
    const bytes = new Uint8Array(await file.arrayBuffer()); if (epoch !== documentEpoch) return;
    source = { bytes, password: new Uint8Array(), recovery: $('strict').checked ? 0 : 1 };
    const saved = source;
    client = createClient(); const current = client; await current.hello();
    let password = new Uint8Array(), retried = false, opened;
    for (;;) {
      const attemptedPassword = password;
      try { opened = await current.open(bytes, saved.recovery, password); saved.password = password.slice(); break; }
      catch (error) {
        if (epoch !== documentEpoch) return;
        if (!(error instanceof HostError) || error.category !== 6) throw error;
        password.fill(0);
        password = await requestPassword(retried ? 'Incorrect password. Try again.' : 'This PDF requires a password.');
        if (epoch !== documentEpoch) { password?.fill(0); return; }
        if (password === null) { status.textContent = 'Opening cancelled'; return; }
        retried = true;
      } finally { attemptedPassword.fill(0); }
      if (epoch !== documentEpoch) return;
    }
    if (epoch !== documentEpoch) return;
    handle = opened.document; layout = new PageLayout(opened.pages); controls(); void reader.open(); void editor.open();
    if (!opened.pages) { status.textContent = 'Document has no pages'; return; }
    await pageInfo(0, epoch); if (epoch !== documentEpoch) return;
    zoom = zoomInput.value.startsWith('fit-') ? fit(zoomInput.value, infos.get(0)) : zoomInput.value === 'custom' ? zoom : Number(zoomInput.value);
    if (!Number.isFinite(zoom)) zoom = 1;
    zoom = Math.min(8, Math.max(0.1, zoom)); updateLayout(); viewport.scrollTop = 0; viewport.scrollLeft = 0; invalidate();
  } catch (error) { if (epoch === documentEpoch) report(error); }
}
$('rotate-view').addEventListener('click', () => {
  const info = infos.get(page); if (!info?.width) return;
  rotations.set(page, ((rotations.get(page) ?? 0) + 270) % 360);
  layout.setHeight(page, displaySize(info).height);
  // CSS rotates the complete page, including hit targets; existing pixels are reusable.
  if (zoomInput.value.startsWith('fit-')) changeZoom(fit(zoomInput.value, info), 0);
  updateLayout(); viewport.scrollTop = layout.top(page, zoom); thumbnails.refresh(); invalidate();
});
$('sidebar-toggle').addEventListener('click', () => {
  const sidebar = $('document-sidebar'); sidebar.hidden = !sidebar.hidden;
  $('sidebar-toggle').setAttribute('aria-expanded', String(!sidebar.hidden)); thumbnails.select(page, true);
});
function sidebarTab(outline) {
  $('thumbnails-panel').hidden = outline; $('outline-sidebar').hidden = !outline;
  for (const [id, selected] of [['thumbnails-tab', !outline], ['outline-tab', outline]]) {
    $(id).setAttribute('aria-selected', String(selected)); $(id).tabIndex = selected ? 0 : -1;
  }
  thumbnails.select(page, true);
}
$('thumbnails-tab').addEventListener('click', () => sidebarTab(false));
$('outline-tab').addEventListener('click', () => sidebarTab(true));
for (const id of ['thumbnails-tab', 'outline-tab']) $(id).addEventListener('keydown', event => {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
  event.preventDefault(); const outline = event.key === 'End' || (event.key !== 'Home' && id === 'thumbnails-tab');
  sidebarTab(outline); $(outline ? 'outline-tab' : 'thumbnails-tab').focus();
});
$('reading-dismiss').addEventListener('click', () => { $('reading-status').textContent = ''; });
$('diagnostics-open').addEventListener('click', () => $('diagnostics-dialog').showModal());
$('diagnostics-close').addEventListener('click', () => $('diagnostics-dialog').close());
$('diagnostics-dialog').addEventListener('close', () => $('more-menu').querySelector('summary').focus());
$('more-menu').addEventListener('toggle', () => { $('more-menu').querySelector('summary').setAttribute('aria-expanded', String($('more-menu').open)); });
$('more-menu').addEventListener('click', event => { if (event.target.closest('button')) $('more-menu').open = false; });
document.addEventListener('click', event => { if (!event.target.closest('#more-menu')) $('more-menu').open = false; });
document.addEventListener('keydown', event => { if (event.key === 'Escape' && $('more-menu').open) { $('more-menu').open = false; $('more-menu').querySelector('summary').focus(); } });
$('previous').addEventListener('click', () => { void navigate(page - 1); });
$('next').addEventListener('click', () => { void navigate(page + 1); });
pageInput.addEventListener('change', () => { void navigate(pageInput.value.startsWith('#') ? Number(pageInput.value.slice(1)) - 1 : reader.pageIndex(pageInput.value)); });
zoomInput.addEventListener('change', () => {
  const info = infos.get(page); if (!info?.width) return;
  changeZoom(zoomInput.value.startsWith('fit-') ? fit(zoomInput.value, info) : Number(zoomInput.value));
});
$('file').addEventListener('change', (event) => { void openFile(event.target.files[0]); event.target.value = ''; });
window.addEventListener('dragover', (event) => { event.preventDefault(); document.body.classList.add('dragging'); });
window.addEventListener('dragleave', (event) => { if (!event.relatedTarget) document.body.classList.remove('dragging'); });
window.addEventListener('drop', (event) => { event.preventDefault(); document.body.classList.remove('dragging'); void openFile(event.dataTransfer.files[0]); });
viewport.addEventListener('scroll', () => {
  const dx = viewport.scrollLeft - lastScroll[0], dy = viewport.scrollTop - lastScroll[1];
  if (dx || dy) direction = Math.abs(dy) >= Math.abs(dx) ? { axis: 'y', sign: Math.sign(dy) } : { axis: 'x', sign: Math.sign(dx) };
  lastScroll = [viewport.scrollLeft, viewport.scrollTop];
  if (layout.count) { page = layout.dominantPage(viewport.scrollTop, viewport.clientHeight, zoom, page); controls(); }
  invalidate();
}, { passive: true });
viewport.addEventListener('wheel', (event) => {
  if (!event.ctrlKey && !event.metaKey) return;
  event.preventDefault(); zoomInput.value = 'custom';
  const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? viewport.clientHeight : 1);
  changeZoom(zoom * Math.exp(-Math.max(-1000, Math.min(1000, delta)) * 0.002), event.clientY - viewport.getBoundingClientRect().top);
}, { passive: false });
let pinch = null;
viewport.addEventListener('touchstart', (event) => {
  if (event.touches.length !== 2) return;
  event.preventDefault(); const [a, b] = event.touches;
  pinch = { distance: Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY), zoom };
}, { passive: false });
viewport.addEventListener('touchmove', (event) => {
  if (!pinch || event.touches.length !== 2) return;
  event.preventDefault(); const [a, b] = event.touches; zoomInput.value = 'custom';
  if (pinch.distance > 0) changeZoom(pinch.zoom * Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY) / pinch.distance,
    (a.clientY + b.clientY) / 2 - viewport.getBoundingClientRect().top);
}, { passive: false });
for (const type of ['touchend', 'touchcancel']) viewport.addEventListener(type, () => { pinch = null; });
window.addEventListener('keydown', (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') { event.preventDefault(); if (handle !== null) void editor.save(); return; }
  if (['INPUT', 'SELECT', 'TEXTAREA', 'BUTTON'].includes(event.target?.tagName) || event.target?.closest?.('[role=tree]') || event.altKey) return;
  if (['PageDown', 'PageUp', 'Home', 'End'].includes(event.key)) {
    event.preventDefault(); void navigate(event.key === 'Home' ? 0 : event.key === 'End' ? layout.count - 1 : page + (event.key === 'PageDown' ? 1 : -1));
  } else if (['+', '=', '-', '0'].includes(event.key)) {
    event.preventDefault(); zoomInput.value = event.key === '0' ? '1' : 'custom'; changeZoom(event.key === '0' ? 1 : zoom * (event.key === '-' ? 1 / 1.2 : 1.2));
  }
});
let quietTimer;
function scheduleQuietWork() {
  clearTimeout(quietTimer);
  quietTimer = setTimeout(() => { planAhead(); thumbnails.schedule(); }, Math.max(0, 550 - (performance.now() - lastInteraction)));
}
function noteInteraction() {
  lastInteraction = performance.now(); preloader.cancel(); scheduleQuietWork();
}
for (const type of ['input', 'keydown', 'focusin']) document.addEventListener(type, noteInteraction, true);
document.addEventListener('pointerdown', () => { pointerDown = true; noteInteraction(); }, true);
for (const type of ['pointerup', 'pointercancel']) document.addEventListener(type, () => { pointerDown = false; noteInteraction(); }, true);
document.addEventListener('focusout', noteInteraction, true);
document.addEventListener('visibilitychange', noteInteraction);
let ratio = window.devicePixelRatio;
function resizeViewport() {
  if (handle === null) return;
  const info = infos.get(page);
  if (zoomInput.value.startsWith('fit-') && info?.width) {
    const value = Math.min(8, Math.max(0.1, fit(zoomInput.value, info)));
    if (value === zoom) invalidate(); else changeZoom(value);
  } else if (ratio !== window.devicePixelRatio) changeZoom(zoom);
  else invalidate();
  ratio = window.devicePixelRatio; thumbnails.refresh();
}
window.addEventListener('resize', resizeViewport);
// Search/outline panels resize the document region without resizing the window.
const regionObserver = typeof ResizeObserver === 'function' ? new ResizeObserver(() => requestAnimationFrame(resizeViewport)) : null;
regionObserver?.observe(viewport);
window.addEventListener('beforeunload', (event) => { if (editor.changes.size) { event.preventDefault(); event.returnValue = ''; } });
window.addEventListener('pagehide', () => { clearTimeout(quietTimer); preloader.destroy(); tileCache.clear(); thumbnails.destroy(); ++documentEpoch; handle = null; dirty = false; regionObserver?.disconnect(); reader.reset(); services.reset(); source?.password.fill(0); source = null; client?.destroy(); });
if (debug) window.paguraDebug = Object.freeze({ tileCache: () => ({ ...tileCache.stats(), preloaded: preloader.completed, discarded: preloader.discarded, pending: preloader.jobs.length }), fail: (reason) => client.injectFailure(reason),
  readingProfile: async () => {
    const { profileTextLayer } = await import('./reading_profile.mjs');
    const epoch = documentEpoch, text = await client.pageText(handle, page), info = await pageInfo(page, epoch);
    if (epoch !== documentEpoch) throw new Error('Document changed during profiling');
    return { characters: text.characters.length, samplesMs: profileTextLayer(text, info), accounting: await client.readingStats() };
  },
});
let startupEpoch = documentEpoch;
try {
  client = createClient(); await client.hello(); status.textContent = 'Ready — open a PDF';
  const values = new URL(window.location.href).searchParams.getAll('file');
  if (!values.length && new URL(window.location.href).searchParams.get('demo') === 'form') {
    const { formDocument } = await import('./form_document.mjs');
    await openFile(new File([formDocument()], 'sample-form.pdf', { type: 'application/pdf' }));
  }
  if (values.length) {
    if (values.length !== 1) throw new Error('Startup file must be specified once');
    const epoch = documentEpoch; startupEpoch = epoch; status.textContent = 'Loading fixture…';
    const file = await loadFixture(values[0], window.location.href);
    if (epoch === documentEpoch) await openFile(file);
  }
} catch (error) { if (startupEpoch === documentEpoch) report(error); }
