import { SessionClient } from './client.mjs';
import { MAX_FILE } from './protocol.mjs';
import { loadFixture } from './fixtures.mjs';

const element = (id) => document.getElementById(id);
const surface = element('page-surface'), viewport = element('viewport');
const status = element('status'), errorBox = element('error');
const pageInput = element('page'), zoomInput = element('zoom');
const TILE = 256, MAX_CACHED_TILES = 128, MAX_EXTENT = 10_000_000;
let client, handle = null, pages = 0, page = 0, generation = 0, opening = 0;
let geometry = null, scale = 1, deviceWidth = 0, deviceHeight = 0;
let tiles = new Map(), drawing = false, rerender = false, failed = false;

function report(error) {
  errorBox.textContent = String(error); errorBox.hidden = false;
  status.textContent = 'Page unavailable';
}
function controls() {
  pageInput.disabled = !pages; zoomInput.disabled = !pages;
  element('previous').disabled = !pages || page <= 0;
  element('next').disabled = !pages || page >= pages - 1;
  pageInput.max = String(pages); pageInput.value = String(page + 1);
  element('count').textContent = `of ${pages}`;
}
function clearPage() {
  generation++; tiles.clear(); surface.replaceChildren(); surface.hidden = true;
  surface.removeAttribute('data-failed'); failed = false; geometry = null;
}
function failPage(error, token) {
  if (token !== generation) return;
  failed = true; tiles.clear(); surface.replaceChildren();
  surface.setAttribute('data-failed', ''); report(error);
}
async function repairs(documentHandle, token) {
  const diagnostics = await client.diagnostics(documentHandle);
  if (token !== generation) return;
  element('diagnostic-count').textContent = `Repairs: ${diagnostics.length}`;
  element('repair-list').replaceChildren(...diagnostics.map((item) => {
    const li = document.createElement('li');
    const location = item.object ? ` (object ${item.object.number} ${item.object.generation})` : '';
    li.textContent = `[${item.tag}] ${item.message}${location}`; return li;
  }));
  element('diagnostics').open = diagnostics.length > 0;
}
async function openFile(file) {
  if (!file) return;
  const attempt = ++opening;
  clearPage(); handle = null; pages = 0; controls();
  errorBox.hidden = true; status.textContent = 'Opening…'; element('filename').textContent = file.name;
  element('diagnostic-count').textContent = 'Repairs: 0'; element('repair-list').replaceChildren();
  try {
    if (file.size > MAX_FILE) throw new Error('PDF exceeds the 512 MiB input budget');
    // One document per viewer: restart the worker to release old caches and any
    // in-flight request. Protocol sessions themselves support multiple documents.
    client?.destroy(); client = createClient();
    const current = client;
    await current.hello();
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (attempt !== opening) return;
    const result = await current.open(bytes, element('strict').checked ? 0 : 1);
    if (attempt !== opening) return;
    handle = result.document; pages = result.pages; page = 0; controls();
    if (!pages) { status.textContent = 'Document has no pages'; return; }
    await showPage();
  } catch (error) { if (attempt === opening) report(error); }
}
async function showPage() {
  if (handle === null) return;
  clearPage(); const token = generation, documentHandle = handle;
  errorBox.hidden = true; status.textContent = 'Loading page…'; controls();
  try {
    geometry = await client.pageInfo(documentHandle, page);
    if (token !== generation) return;
    scale = Number(zoomInput.value) * window.devicePixelRatio;
    // Use the same unrounded physical geometry as device_size, not its rounded
    // scale-1 dimensions; rotation swaps the physical extents.
    let width = (geometry.crop[2] - geometry.crop[0]) * geometry.userUnit;
    let height = (geometry.crop[3] - geometry.crop[1]) * geometry.userUnit;
    if (geometry.rotation === 90 || geometry.rotation === 270) [width, height] = [height, width];
    deviceWidth = Math.max(1, Math.ceil(width * scale - 1e-9));
    deviceHeight = Math.max(1, Math.ceil(height * scale - 1e-9));
    if (!Number.isFinite(deviceWidth) || !Number.isFinite(deviceHeight) || deviceWidth > MAX_EXTENT || deviceHeight > MAX_EXTENT) {
      throw new Error('Page exceeds the viewer layout budget; reduce zoom');
    }
    surface.style.width = `${deviceWidth / window.devicePixelRatio}px`;
    surface.style.height = `${deviceHeight / window.devicePixelRatio}px`;
    surface.hidden = false; viewport.scrollTop = 0; viewport.scrollLeft = 0;
    scheduleDraw(); await repairs(documentHandle, token);
  } catch (error) { failPage(error, token); }
}
function visibleTiles() {
  const bounds = surface.getBoundingClientRect(), view = viewport.getBoundingClientRect();
  const ratio = window.devicePixelRatio;
  const left = Math.max(0, Math.floor((view.left - bounds.left) * ratio / TILE));
  const top = Math.max(0, Math.floor((view.top - bounds.top) * ratio / TILE));
  const right = Math.min(Math.ceil(deviceWidth / TILE), Math.ceil((view.right - bounds.left) * ratio / TILE));
  const bottom = Math.min(Math.ceil(deviceHeight / TILE), Math.ceil((view.bottom - bounds.top) * ratio / TILE));
  const visible = [];
  for (let row = top; row < bottom; row++) for (let column = left; column < right; column++) visible.push({ x: column * TILE, y: row * TILE });
  // Bound work even on abnormally large viewports. Normal display sizes fit.
  if (visible.length > MAX_CACHED_TILES) throw new Error('Visible page exceeds the tile budget; use a smaller window');
  return visible;
}
async function drawVisible() {
  if (drawing) { rerender = true; return; }
  if (surface.hidden || !geometry || failed || handle === null) return;
  drawing = true;
  const token = generation, documentHandle = handle, pageIndex = page, renderScale = scale;
  try {
    const visible = visibleTiles(), visibleKeys = new Set(visible.map(({ x, y }) => `${x},${y}`));
    // Evict offscreen canvases first; retained pixel memory is bounded by tile count.
    for (const [key, canvas] of tiles) if (!visibleKeys.has(key)) { canvas.remove(); tiles.delete(key); }
    for (const { x, y } of visible) {
      if (token !== generation) break;
      const key = `${x},${y}`;
      if (tiles.has(key)) continue;
      status.textContent = 'Rendering visible tiles…';
      const tile = await client.renderTile(documentHandle, pageIndex, renderScale,
        { x, y, width: Math.min(TILE, deviceWidth - x), height: Math.min(TILE, deviceHeight - y) });
      if (token !== generation) break;
      const canvas = document.createElement('canvas'); canvas.className = 'tile'; canvas.width = tile.width; canvas.height = tile.height;
      const ratio = window.devicePixelRatio;
      Object.assign(canvas.style, { left: `${x / ratio}px`, top: `${y / ratio}px`, width: `${tile.width / ratio}px`, height: `${tile.height / ratio}px` });
      const context = canvas.getContext('2d');
      if (!context) throw new Error('Canvas 2D rendering is unavailable');
      context.putImageData(new ImageData(new Uint8ClampedArray(tile.rgba.buffer, tile.rgba.byteOffset, tile.rgba.byteLength), tile.width, tile.height), 0, 0);
      tiles.set(key, canvas); surface.append(canvas);
    }
    if (token === generation) { status.textContent = 'Visible tiles rendered'; await repairs(documentHandle, token); }
  } catch (error) { failPage(error, token); }
  finally { drawing = false; if (rerender) { rerender = false; scheduleDraw(); } }
}
let scheduled = false;
function scheduleDraw() {
  if (scheduled) return;
  scheduled = true; requestAnimationFrame(() => { scheduled = false; void drawVisible(); });
}
function navigate(value) {
  if (!Number.isInteger(value) || value < 0 || value >= pages) { controls(); return; }
  page = value; void showPage();
}
element('previous').addEventListener('click', () => navigate(page - 1));
element('next').addEventListener('click', () => navigate(page + 1));
pageInput.addEventListener('change', () => navigate(Number(pageInput.value) - 1));
zoomInput.addEventListener('change', () => { if (handle !== null) void showPage(); });
element('file').addEventListener('change', (event) => { void openFile(event.target.files[0]); event.target.value = ''; });
window.addEventListener('dragover', (event) => { event.preventDefault(); document.body.classList.add('dragging'); });
window.addEventListener('dragleave', (event) => { if (!event.relatedTarget) document.body.classList.remove('dragging'); });
window.addEventListener('drop', (event) => { event.preventDefault(); document.body.classList.remove('dragging'); void openFile(event.dataTransfer.files[0]); });
viewport.addEventListener('scroll', scheduleDraw, { passive: true });
let pixelRatio = window.devicePixelRatio;
window.addEventListener('resize', () => {
  if (pixelRatio !== window.devicePixelRatio) { pixelRatio = window.devicePixelRatio; if (handle !== null) void showPage(); }
  else scheduleDraw();
});
window.addEventListener('pagehide', () => client?.destroy());

function createClient() {
  if (!crossOriginIsolated || typeof SharedArrayBuffer !== 'function') throw new Error('Cross-origin isolation is required. Start python3 tools/serve_web.py and open http://localhost:8000.');
  return new SessionClient(new Worker(new URL('./worker.mjs', import.meta.url), { type: 'module' }));
}

async function openStartupFixture() {
  const values = new URL(window.location.href).searchParams.getAll('file');
  if (!values.length) return;
  const attempt = opening;
  try {
    if (values.length !== 1) throw new Error('Startup file must be specified once');
    status.textContent = 'Loading fixture…';
    const file = await loadFixture(values[0], window.location.href);
    // A picker/drop during the fetch takes precedence over startup loading.
    if (attempt === opening) await openFile(file);
  } catch (error) { if (attempt === opening) report(error); }
}

try {
  client = createClient();
  await client.hello(); status.textContent = 'Ready — open a PDF';
  await openStartupFixture();
} catch (error) { report(error); }
