import { pageUserPoint } from './page_geometry.mjs';
import { ReadingView } from './reading_view.mjs';
import { destinationView } from './destinations.mjs';
import { SessionClient } from './client.mjs';
import { MAX_FILE, HostError } from './protocol.mjs';
import { loadFixture } from './fixtures.mjs';
import { PageLayout } from './layout.mjs';
import { requestPassword, cancelPassword } from './password.mjs';

const $ = (id) => document.getElementById(id);
const viewport = $('viewport'),
  surface = $('page-surface'),
  status = $('status'),
  errorBox = $('error');
const pageInput = $('page'),
  zoomInput = $('zoom');
const TILE = 256,
  MAX_TILES = 128,
  MAX_PAGES = 32,
  MAX_EXTENT = 10000000;
let client,
  handle = null,
  layout = new PageLayout(0),
  page = 0,
  zoom = 1;
let documentEpoch = 0,
  generation = 0,
  drawing = false,
  scheduled = false,
  dirty = false;
const infos = new Map(),
  views = new Map();
const debug =
  new URL(window.location.href).searchParams.get('debug') === '1' &&
  ['localhost', '127.0.0.1', '[::1]'].includes(new URL(window.location.href).hostname);
let source = null,
  recoveryAttempts = 0,
  recovering = false;

const reader = new ReadingView({
  state: () => ({
    client,
    handle,
    epoch: documentEpoch,
    page,
    count: layout.count,
    zoom,
    scrollMatch: (index, y) => {
      viewport.scrollTop = layout.top(index, zoom) + Math.max(0, y - 40);
      invalidate();
    },
  }),
  navigate,
  destination: followDestination,
  report: (error) => {
    $('reading-status').textContent = String(error);
  },
  controls,
  notice: readingNotice,
});
function readingNotice(message) {
  $('reading-status').textContent = message;
}
function report(error) {
  errorBox.textContent = String(error);
  errorBox.hidden = false;
  status.textContent = 'Page unavailable';
}
function controls() {
  const count = layout.count;
  pageInput.disabled = !count;
  zoomInput.disabled = !count;
  $('previous').disabled = !count || page === 0;
  $('next').disabled = !count || page === count - 1;
  pageInput.max = String(count);
  if (document.activeElement !== pageInput) pageInput.value = String(page + 1);
  $('count').textContent = `of ${count}`;
  $('page-label').textContent = count ? `Label: ${reader.label(page)}` : '';
  $('copy-page').disabled = !count;
}
function createClient() {
  if (!crossOriginIsolated || typeof SharedArrayBuffer !== 'function')
    throw new Error(
      'Cross-origin isolation is required. Start python3 tools/serve_web.py and open http://localhost:8000.',
    );
  const current = new SessionClient(new Worker(new URL('./worker.mjs', import.meta.url), { type: 'module' }), {
    debug,
    onFailure: (error) => {
      if (client === current) void recover(error);
    },
  });
  return current;
}
async function recover(error) {
  if (recovering) return;
  if (!source || recoveryAttempts >= 1) {
    handle = null;
    client?.destroy();
    for (const view of views.values()) view.element.remove();
    views.clear();
    report(`${error}. Automatic recovery stopped; reopen the file to retry.`);
    return;
  }
  recovering = true;
  recoveryAttempts++;
  const saved = source,
    position = anchor(),
    epoch = ++documentEpoch;
  handle = null;
  infos.clear();
  views.clear();
  surface.replaceChildren();
  report(`${error}. Restarting the PDF host…`);
  status.textContent = 'Recovering document…';
  try {
    client?.destroy();
    client = createClient();
    const current = client;
    await current.hello();
    const opened = await current.open(saved.bytes, saved.recovery, saved.password);
    if (epoch !== documentEpoch) return;
    handle = opened.document;
    page = Math.min(position.index, Math.max(0, opened.pages - 1));
    if (layout.count !== opened.pages) layout = new PageLayout(opened.pages);
    if (opened.pages) {
      await pageInfo(page, epoch);
      if (epoch !== documentEpoch) return;
    }
    updateLayout();
    if (opened.pages) restore({ ...position, index: page });
    controls();
    void reader.open();
    errorBox.textContent = `${error}. PDF host restarted; document reopened at page ${page + 1}, ${Math.round(zoom * 100)}%.`;
    errorBox.hidden = false;
    invalidate();
  } catch (failure) {
    if (epoch === documentEpoch) {
      handle = null;
      client?.destroy();
      report(`${failure}. Automatic recovery stopped; reopen the file to retry.`);
    }
  } finally {
    recovering = false;
  }
}
function invalidate() {
  if (handle !== null) {
    generation = client.invalidate(handle);
    status.textContent = 'Updating visible pages…';
  }
  dirty = true;
  schedule();
}
function schedule() {
  if (scheduled) return;
  scheduled = true;
  requestAnimationFrame(() => {
    scheduled = false;
    void draw();
  });
}
function anchor(point = 0) {
  const index = layout.pageAt(viewport.scrollTop + point, zoom);
  return {
    index,
    fraction: (viewport.scrollTop + point - layout.top(index, zoom)) / (layout.heights[index] * zoom || 1),
    point,
  };
}
function restore(position) {
  viewport.scrollTop = Math.max(
    0,
    layout.top(position.index, zoom) + position.fraction * layout.heights[position.index] * zoom - position.point,
  );
}
function updateLayout() {
  const height = layout.height(zoom);
  if (!Number.isFinite(height) || height > MAX_EXTENT)
    throw new Error('Document exceeds the viewer layout budget; reduce zoom');
  let width = Math.max(1, viewport.clientWidth - 48);
  for (const info of infos.values()) if (info.width) width = Math.max(width, info.width * zoom);
  if (width > MAX_EXTENT) throw new Error('Page exceeds the viewer layout budget; reduce zoom');
  surface.style.height = `${height}px`;
  surface.style.width = `${width}px`;
  surface.hidden = !layout.count;
  for (const [index, view] of views) {
    const info = infos.get(index);
    Object.assign(view.element.style, {
      top: `${layout.top(index, zoom)}px`,
      width: `${(info?.width ?? 612) * zoom}px`,
      height: `${layout.heights[index] * zoom}px`,
    });
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
  const promise = client
    .pageInfo(handle, index)
    .then((raw) => {
      if (epoch !== documentEpoch) return null;
      const position = anchor(),
        info = geometry(raw);
      const offset = position.fraction * layout.heights[position.index] * zoom;
      infos.set(index, info);
      layout.setHeight(index, info.height);
      updateLayout();
      // Newly measured heights keep the same pixel offset in the anchor page;
      // zoom changes instead retain a proportional position in known geometry.
      position.fraction =
        Math.min(offset, layout.heights[position.index] * zoom + layout.gap - 1) /
        (layout.heights[position.index] * zoom);
      restore(position);
      // Retain compact heights for every page, but bound detailed metadata.
      if (infos.size > 256)
        for (const key of infos.keys()) {
          if (!views.has(key) && key !== index) {
            infos.delete(key);
            break;
          }
        }
      return info;
    })
    .catch((error) => {
      if (epoch === documentEpoch) infos.delete(index);
      throw error;
    });
  infos.set(index, { promise });
  return promise;
}
function pageView(index) {
  if (views.has(index)) return views.get(index);
  const element = document.createElement('section');
  element.className = 'pdf-page';
  element.setAttribute('data-page', String(index));
  element.setAttribute('role', 'region');
  element.setAttribute('aria-label', `Page ${index + 1}`);
  element.setAttribute('aria-busy', 'true');
  const message = document.createElement('p');
  message.className = 'page-message';
  message.textContent = `Loading page ${index + 1}…`;
  element.append(message);
  let following = null,
    nextIndex = Infinity;
  for (const [number, view] of views)
    if (number > index && number < nextIndex) {
      following = view.element;
      nextIndex = number;
    }
  surface.insertBefore(element, following);
  const view = { element, message, tiles: new Map(), failed: false };
  views.set(index, view);
  return view;
}
function visiblePages() {
  const top = Math.max(0, viewport.scrollTop - 24),
    bottom = top + viewport.clientHeight;
  const first = layout.pageAt(top, zoom),
    last = layout.pageAt(bottom, zoom);
  if (last - first + 1 > MAX_PAGES) throw new Error('Too many visible pages; increase zoom');
  return Array.from({ length: last - first + 1 }, (_, offset) => first + offset);
}
function tileRequests(index, info) {
  const ratio = window.devicePixelRatio,
    scale = zoom * ratio;
  const width = Math.max(1, Math.ceil(info.width * scale - 1e-9)),
    height = Math.max(1, Math.ceil(info.height * scale - 1e-9));
  const bounds = views.get(index).element.getBoundingClientRect(),
    visible = viewport.getBoundingClientRect();
  const left = Math.max(0, Math.floor(((visible.left - bounds.left) * ratio) / TILE) * TILE);
  const top = Math.max(0, Math.floor(((visible.top - bounds.top) * ratio) / TILE) * TILE);
  const right = Math.min(width, Math.ceil((visible.right - bounds.left) * ratio));
  const bottom = Math.min(height, Math.ceil((visible.bottom - bounds.top) * ratio));
  const requests = [];
  for (let y = top; y < bottom; y += TILE)
    for (let x = left; x < right; x += TILE) {
      if (requests.length >= MAX_TILES) throw new Error('Visible page exceeds the tile budget; use a smaller window');
      requests.push({ x, y, width: Math.min(TILE, width - x), height: Math.min(TILE, height - y) });
    }
  return requests;
}
function failPage(index, error) {
  const view = views.get(index);
  if (!view) return;
  view.failed = true;
  view.tiles.clear();
  view.element.replaceChildren(view.message);
  view.message.hidden = false;
  view.message.setAttribute('role', 'alert');
  view.message.textContent = String(error);
  view.element.setAttribute('aria-busy', 'false');
  report(`Page ${index + 1}: ${error}`);
}
async function diagnostics(epoch) {
  const items = await client.diagnostics(handle);
  if (epoch !== documentEpoch) return;
  const repairs = items.filter((item) => item.kind === 'repair').length;
  $('diagnostic-count').textContent = `Repairs: ${repairs} · Information: ${items.length - repairs}`;
  $('repair-list').replaceChildren(
    ...items.map((item) => {
      const li = document.createElement('li');
      li.textContent = `${item.kind === 'repair' ? 'Repair' : 'Information'} [${item.tag}] ${item.message}`;
      return li;
    }),
  );
  $('diagnostics').open = items.length > 0;
}
async function draw() {
  if (drawing || handle === null || !layout.count) return;
  drawing = true;
  dirty = false;
  const epoch = documentEpoch,
    token = generation,
    documentHandle = handle;
  try {
    // Heights become precise only as the visible range is loaded.
    for (let pass = 0; pass < MAX_PAGES; pass++) {
      const indexes = visiblePages();
      reader.prune(indexes);
      for (const [index, view] of views)
        if (!indexes.includes(index)) {
          view.element.remove();
          views.delete(index);
        }
      for (const index of indexes) pageView(index);
      const missing = indexes.find((index) => !infos.get(index)?.width);
      if (missing === undefined) break;
      await pageInfo(missing, epoch);
      if (epoch !== documentEpoch || token !== generation) return;
    }
    updateLayout();
    let total = 0;
    for (const index of visiblePages()) {
      if (epoch !== documentEpoch || token !== generation) return;
      const info = infos.get(index),
        view = pageView(index);
      if (!info?.width || view.failed) continue;
      const requests = tileRequests(index, info);
      total += requests.length;
      if (total > MAX_TILES) throw new Error('Visible pages exceed the tile budget; increase zoom');
      const keys = new Set(requests.map(({ x, y }) => `${x},${y}`));
      for (const [key, canvas] of view.tiles)
        if (!keys.has(key)) {
          canvas.remove();
          view.tiles.delete(key);
        }
      try {
        for (const tile of requests) {
          if (epoch !== documentEpoch || token !== generation) return;
          const key = `${tile.x},${tile.y}`;
          if (view.tiles.has(key)) continue;
          status.textContent = 'Rendering visible tiles…';
          const image = await client.renderGenerationTile(
            documentHandle,
            index,
            zoom * window.devicePixelRatio,
            tile,
            token,
          );
          if (epoch !== documentEpoch || token !== generation || image.cancelled) return;
          const canvas = document.createElement('canvas');
          canvas.className = 'tile';
          canvas.width = image.width;
          canvas.height = image.height;
          canvas.setAttribute('aria-hidden', 'true');
          const ratio = window.devicePixelRatio;
          Object.assign(canvas.style, {
            left: `${tile.x / ratio}px`,
            top: `${tile.y / ratio}px`,
            width: `${image.width / ratio}px`,
            height: `${image.height / ratio}px`,
          });
          const context = canvas.getContext('2d');
          if (!context) throw new Error('Canvas 2D rendering is unavailable');
          context.putImageData(
            new ImageData(
              new Uint8ClampedArray(image.rgba.buffer, image.rgba.byteOffset, image.rgba.byteLength),
              image.width,
              image.height,
            ),
            0,
            0,
          );
          view.tiles.set(key, canvas);
          view.element.append(canvas);
        }
        view.message.hidden = true;
        view.element.setAttribute('aria-busy', 'false');
        await reader.page(index, info, view.element);
        if (epoch !== documentEpoch || token !== generation) return;
      } catch (error) {
        if (epoch === documentEpoch && token === generation) failPage(index, error);
      }
    }
    if (epoch === documentEpoch && token === generation) {
      await diagnostics(epoch);
      if (epoch === documentEpoch && token === generation) {
        controls();
        status.textContent = [...views.values()].some((view) => view.failed)
          ? 'Some pages are unavailable'
          : 'Visible tiles rendered';
      }
    }
  } catch (error) {
    if (epoch === documentEpoch && token === generation) report(error);
  } finally {
    drawing = false;
    if (dirty) schedule();
  }
}
function fit(mode, info) {
  const width = (viewport.clientWidth - 48) / info.width;
  return mode === 'fit-page' ? Math.min(width, (viewport.clientHeight - 48) / info.height) : width;
}
function changeZoom(value, point = viewport.clientHeight / 2) {
  if (!Number.isFinite(value) || handle === null) return;
  const position = anchor(point),
    previousZoom = zoom;
  zoom = Math.min(8, Math.max(0.1, value));
  try {
    updateLayout();
  } catch (error) {
    zoom = previousZoom;
    report(error);
    return;
  }
  for (const view of views.values()) view.element.remove();
  views.clear();
  reader.prune([]);
  $('custom-zoom').textContent = `${Math.round(zoom * 100)}%`;
  restore(position);
  invalidate();
}
async function navigate(index) {
  if (!Number.isInteger(index) || index < 0 || index >= layout.count || handle === null) {
    pageInput.value = String(page + 1);
    return;
  }
  const epoch = documentEpoch;
  status.textContent = 'Loading page…';
  try {
    const info = await pageInfo(index, epoch);
    if (!info || epoch !== documentEpoch) return;
    page = index;
    if (zoomInput.value.startsWith('fit-')) changeZoom(fit(zoomInput.value, info));
    viewport.scrollTop = layout.top(index, zoom);
    pageInput.value = String(index + 1);
    controls();
    invalidate();
  } catch (error) {
    if (epoch === documentEpoch) report(error);
  }
}
async function followDestination(action) {
  const epoch = documentEpoch,
    previous = infos.get(page);
  const retained = previous?.width
    ? pageUserPoint(
        previous,
        zoom,
        viewport.scrollLeft - (parseFloat(surface.style.width) - previous.width * zoom) / 2,
        viewport.scrollTop - layout.top(page, zoom),
      )
    : null;
  const info = await pageInfo(action.page, epoch);
  if (!info || epoch !== documentEpoch) return;
  const bounds = action.fit >= 5 ? (await client.contentBounds(handle, action.page)).bounds : null;
  if (epoch !== documentEpoch) return;
  const plan = destinationView(
    action,
    info,
    viewport.clientWidth - 48,
    viewport.clientHeight - 48,
    zoom,
    retained ? { left: retained[0], top: retained[1] } : {},
    bounds,
  );
  zoomInput.value = 'custom';
  changeZoom(plan.zoom, 0);
  await navigate(action.page);
  if (epoch !== documentEpoch) return;
  if (plan.y !== null) viewport.scrollTop = layout.top(action.page, zoom) + plan.y;
  if (plan.x !== null)
    viewport.scrollLeft = Math.max(0, (parseFloat(surface.style.width) - info.width * zoom) / 2 + plan.x);
  invalidate();
  if (plan.notice) readingNotice(plan.notice);
}
async function openFile(file) {
  if (!file) return;
  cancelPassword();
  source?.password.fill(0);
  source = null;
  recoveryAttempts = 0;
  const epoch = ++documentEpoch;
  reader.reset();
  handle = null;
  infos.clear();
  views.clear();
  surface.replaceChildren();
  surface.hidden = true;
  layout = new PageLayout(0);
  page = 0;
  controls();
  errorBox.hidden = true;
  $('filename').textContent = file.name;
  status.textContent = 'Opening…';
  $('diagnostic-count').textContent = 'Repairs: 0 · Information: 0';
  $('repair-list').replaceChildren();
  try {
    if (file.size > MAX_FILE) throw new Error('PDF exceeds the 512 MiB input budget');
    client?.destroy();
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (epoch !== documentEpoch) return;
    source = { bytes, password: new Uint8Array(), recovery: $('strict').checked ? 0 : 1 };
    const saved = source;
    client = createClient();
    const current = client;
    await current.hello();
    let password = new Uint8Array(),
      retried = false,
      opened;
    for (;;) {
      const attemptedPassword = password;
      try {
        opened = await current.open(bytes, saved.recovery, password);
        saved.password = password.slice();
        break;
      } catch (error) {
        if (epoch !== documentEpoch) return;
        if (!(error instanceof HostError) || error.category !== 6) throw error;
        password.fill(0);
        password = await requestPassword(retried ? 'Incorrect password. Try again.' : 'This PDF requires a password.');
        if (epoch !== documentEpoch) {
          password?.fill(0);
          return;
        }
        if (password === null) {
          status.textContent = 'Opening cancelled';
          return;
        }
        retried = true;
      } finally {
        attemptedPassword.fill(0);
      }
      if (epoch !== documentEpoch) return;
    }
    if (epoch !== documentEpoch) return;
    handle = opened.document;
    layout = new PageLayout(opened.pages);
    controls();
    void reader.open();
    if (!opened.pages) {
      status.textContent = 'Document has no pages';
      return;
    }
    await pageInfo(0, epoch);
    if (epoch !== documentEpoch) return;
    zoom = zoomInput.value.startsWith('fit-')
      ? fit(zoomInput.value, infos.get(0))
      : zoomInput.value === 'custom'
        ? zoom
        : Number(zoomInput.value);
    if (!Number.isFinite(zoom)) zoom = 1;
    zoom = Math.min(8, Math.max(0.1, zoom));
    updateLayout();
    viewport.scrollTop = 0;
    viewport.scrollLeft = 0;
    invalidate();
  } catch (error) {
    if (epoch === documentEpoch) report(error);
  }
}
$('previous').addEventListener('click', () => {
  void navigate(page - 1);
});
$('next').addEventListener('click', () => {
  void navigate(page + 1);
});
pageInput.addEventListener('change', () => {
  void navigate(
    pageInput.value.startsWith('#') ? Number(pageInput.value.slice(1)) - 1 : reader.pageIndex(pageInput.value),
  );
});
zoomInput.addEventListener('change', () => {
  const info = infos.get(page);
  if (!info?.width) return;
  changeZoom(zoomInput.value.startsWith('fit-') ? fit(zoomInput.value, info) : Number(zoomInput.value));
});
$('file').addEventListener('change', (event) => {
  void openFile(event.target.files[0]);
  event.target.value = '';
});
window.addEventListener('dragover', (event) => {
  event.preventDefault();
  document.body.classList.add('dragging');
});
window.addEventListener('dragleave', (event) => {
  if (!event.relatedTarget) document.body.classList.remove('dragging');
});
window.addEventListener('drop', (event) => {
  event.preventDefault();
  document.body.classList.remove('dragging');
  void openFile(event.dataTransfer.files[0]);
});
viewport.addEventListener(
  'scroll',
  () => {
    if (layout.count) {
      page = layout.dominantPage(viewport.scrollTop, viewport.clientHeight, zoom, page);
      controls();
    }
    invalidate();
  },
  { passive: true },
);
viewport.addEventListener(
  'wheel',
  (event) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    zoomInput.value = 'custom';
    const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? viewport.clientHeight : 1);
    changeZoom(
      zoom * Math.exp(-Math.max(-1000, Math.min(1000, delta)) * 0.002),
      event.clientY - viewport.getBoundingClientRect().top,
    );
  },
  { passive: false },
);
let pinch = null;
viewport.addEventListener(
  'touchstart',
  (event) => {
    if (event.touches.length !== 2) return;
    event.preventDefault();
    const [a, b] = event.touches;
    pinch = { distance: Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY), zoom };
  },
  { passive: false },
);
viewport.addEventListener(
  'touchmove',
  (event) => {
    if (!pinch || event.touches.length !== 2) return;
    event.preventDefault();
    const [a, b] = event.touches;
    zoomInput.value = 'custom';
    if (pinch.distance > 0)
      changeZoom(
        (pinch.zoom * Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY)) / pinch.distance,
        (a.clientY + b.clientY) / 2 - viewport.getBoundingClientRect().top,
      );
  },
  { passive: false },
);
for (const type of ['touchend', 'touchcancel'])
  viewport.addEventListener(type, () => {
    pinch = null;
  });
window.addEventListener('keydown', (event) => {
  if (
    ['INPUT', 'SELECT', 'TEXTAREA', 'BUTTON'].includes(event.target?.tagName) ||
    event.target?.closest?.('[role=tree]') ||
    event.altKey
  )
    return;
  if (['PageDown', 'PageUp', 'Home', 'End'].includes(event.key)) {
    event.preventDefault();
    void navigate(
      event.key === 'Home' ? 0 : event.key === 'End' ? layout.count - 1 : page + (event.key === 'PageDown' ? 1 : -1),
    );
  } else if (['+', '=', '-', '0'].includes(event.key)) {
    event.preventDefault();
    zoomInput.value = event.key === '0' ? '1' : 'custom';
    changeZoom(event.key === '0' ? 1 : zoom * (event.key === '-' ? 1 / 1.2 : 1.2));
  }
});
let ratio = window.devicePixelRatio;
function resizeViewport() {
  if (handle === null) return;
  const info = infos.get(page);
  if (zoomInput.value.startsWith('fit-') && info?.width) {
    const value = Math.min(8, Math.max(0.1, fit(zoomInput.value, info)));
    if (value === zoom) invalidate();
    else changeZoom(value);
  } else if (ratio !== window.devicePixelRatio) changeZoom(zoom);
  else invalidate();
  ratio = window.devicePixelRatio;
}
window.addEventListener('resize', resizeViewport);
// Search/outline panels resize the document region without resizing the window.
const regionObserver =
  typeof ResizeObserver === 'function' ? new ResizeObserver(() => requestAnimationFrame(resizeViewport)) : null;
regionObserver?.observe(viewport);
window.addEventListener('pagehide', () => {
  ++documentEpoch;
  handle = null;
  dirty = false;
  regionObserver?.disconnect();
  reader.reset();
  source?.password.fill(0);
  source = null;
  client?.destroy();
});
if (debug)
  window.paguraDebug = Object.freeze({
    fail: (reason) => client.injectFailure(reason),
    readingProfile: async () => {
      const { profileTextLayer } = await import('./reading_profile.mjs');
      const epoch = documentEpoch,
        text = await client.pageText(handle, page),
        info = await pageInfo(page, epoch);
      if (epoch !== documentEpoch) throw new Error('Document changed during profiling');
      return {
        characters: text.characters.length,
        samplesMs: profileTextLayer(text, info),
        accounting: await client.readingStats(),
      };
    },
  });
let startupEpoch = documentEpoch;
try {
  client = createClient();
  await client.hello();
  status.textContent = 'Ready — open a PDF';
  const values = new URL(window.location.href).searchParams.getAll('file');
  if (values.length) {
    if (values.length !== 1) throw new Error('Startup file must be specified once');
    const epoch = documentEpoch;
    startupEpoch = epoch;
    status.textContent = 'Loading fixture…';
    const file = await loadFixture(values[0], window.location.href);
    if (epoch === documentEpoch) await openFile(file);
  }
} catch (error) {
  if (startupEpoch === documentEpoch) report(error);
}
