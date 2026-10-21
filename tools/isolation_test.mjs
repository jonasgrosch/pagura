import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';
import { prepareIsolation } from '../web/isolation.mjs';

let registrations = 0, reloads = 0;
const script = new URL('../web/isolation_worker.mjs', import.meta.url).href;
const listeners = new Map();
const workers = {
  controller: null,
  async register(url, options) {
    registrations++; assert.equal(url, script); assert.equal(options.type, 'module');
    assert.equal(options.scope, new URL('./', script).pathname);
    assert.equal(options.updateViaCache, 'none');
    this.controller = { scriptURL: script };
  },
  addEventListener: (name, fn) => listeners.set(name, fn),
  removeEventListener: name => listeners.delete(name),
};
const env = { isSecureContext: true, navigator: { serviceWorker: workers }, location: { reload: () => reloads++ } };
assert.equal(await prepareIsolation({ ...env, crossOriginIsolated: true }), true);
assert.equal(registrations, 0, 'already isolated localhost must not register a worker');
assert.equal(await prepareIsolation(env), false); assert.equal(reloads, 1);
assert.equal(listeners.size, 0, 'controller listener is released');
await assert.rejects(prepareIsolation(env), /could not enable/, 'unsupported isolation must not loop reloads');
assert.equal(reloads, 1);
await assert.rejects(prepareIsolation({ isSecureContext: false }), /requires HTTPS/);

const handlers = new Map(); let requests = 0;
runInNewContext(await readFile(new URL('../web/isolation_worker.mjs', import.meta.url), 'utf8'), {
  URL, Headers, Response,
  self: { registration: { scope: 'https://example.test/pagura/' },
    addEventListener: (name, fn) => handlers.set(name, fn),
    skipWaiting: async () => {}, clients: { claim: async () => {} } },
  fetch: async () => { requests++; return new Response('asset', { headers: { 'Content-Type': 'text/javascript' } }); },
});
const dispatch = request => { let reply; handlers.get('fetch')({ request, respondWith: promise => { reply = promise; } }); return reply; };
for (const url of ['https://elsewhere.test/pagura/a', 'https://example.test/another/a', 'https://example.test/pagura-other/a']) {
  assert.equal(dispatch(new Request(url)), undefined);
}
assert.equal(dispatch(new Request('https://example.test/pagura/a', { method: 'POST', body: 'document' })), undefined);
assert.equal(requests, 0, 'uploads, other projects and other origins are not intercepted');
const response = await dispatch(new Request('https://example.test/pagura/worker.mjs'));
assert.equal(response.headers.get('Cross-Origin-Opener-Policy'), 'same-origin');
assert.equal(response.headers.get('Cross-Origin-Embedder-Policy'), 'require-corp');
assert.equal(response.headers.get('Cross-Origin-Resource-Policy'), 'same-origin');
assert.equal(response.headers.get('Content-Type'), 'text/javascript');
assert.match(response.headers.get('Content-Security-Policy'), /connect-src 'self'/);
assert.equal(await response.text(), 'asset');
console.log('Static viewer isolation: scoped headers, first-load bootstrap and reload-loop guard passed');
