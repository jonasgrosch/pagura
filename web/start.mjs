import { prepareIsolation } from './isolation.mjs';
try {
  document.getElementById('status').textContent = 'Preparing viewer…';
  if (await prepareIsolation()) await import('./viewer.mjs');
} catch (error) {
  const notice = document.getElementById('error');
  notice.textContent = String(error); notice.hidden = false;
  document.getElementById('status').textContent = 'Viewer unavailable';
}
