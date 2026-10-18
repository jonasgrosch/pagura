// @ts-check
/** Opt-in development fixture loading. PDF bytes never supply these URLs. */
import { MAX_FILE } from './protocol.mjs';

/**
 * Resolve only same-origin fixture URLs, without traversal or redirects.
 * @param {string} value The `file` query parameter.
 * @param {string} base The viewer URL.
 */
export function fixtureUrl(value, base) {
  const invalid = () =>
    new Error('Startup file must be a same-origin /fixtures/ URL without traversal, credentials, query or fragment');
  if (!value || value !== value.trim()) throw invalid();
  let decoded, url;
  try {
    decoded = decodeURIComponent(value);
    url = new URL(value, base);
  } catch {
    throw invalid();
  }
  if (decoded.includes('\\') || decoded.includes('\0') || decoded.split('/').includes('..')) throw invalid();
  if (
    !['http:', 'https:'].includes(url.protocol) ||
    url.origin !== new URL(base).origin ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    !url.pathname.startsWith('/fixtures/')
  )
    throw invalid();
  const path = decodeURIComponent(url.pathname);
  if (!path.startsWith('/fixtures/') || path.endsWith('/')) throw invalid();
  return url;
}

/**
 * Fetch bounded fixture bytes with no redirect or cross-origin capability.
 * @param {string} value
 * @param {string} base
 */
export async function loadFixture(value, base) {
  const url = fixtureUrl(value, base);
  const response = await fetch(url.href, { mode: 'same-origin', credentials: 'omit', redirect: 'error' });
  if (!response.ok)
    throw new Error(`Cannot load fixture: HTTP ${response.status}. Start tools/serve_web.py with --fixtures.`);
  const declared = response.headers.get('content-length');
  if (declared !== null && (!/^\d+$/.test(declared) || Number(declared) > MAX_FILE)) {
    await response.body?.cancel();
    throw new Error('Fixture exceeds the 512 MiB input budget or has an invalid length');
  }
  if (!response.body) throw new Error('Fixture response has no body');
  const reader = response.body.getReader(),
    chunks = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value: bytes } = await reader.read();
      if (done) break;
      if (bytes.byteLength > MAX_FILE - size) throw new Error('Fixture exceeds the 512 MiB input budget');
      size += bytes.byteLength;
      chunks.push(bytes);
    }
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  } finally {
    reader.releaseLock();
  }
  const name = decodeURIComponent(url.pathname).split('/').at(-1);
  // fixtureUrl requires a path below /fixtures/, so the last segment exists.
  return new File(chunks, /** @type {string} */ (name), { type: 'application/pdf' });
}
