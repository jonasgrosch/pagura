/** Static hosts cannot set COOP/COEP. Establish a scoped worker before opening
 * documents, then reload once under its headers. No document data is persisted. */
export async function prepareIsolation(env = globalThis) {
  if (env.crossOriginIsolated) return true;
  const workers = env.navigator?.serviceWorker;
  if (!env.isSecureContext || !workers) throw new Error('This viewer requires HTTPS and a browser with cross-origin isolation support.');
  const script = new URL('./isolation_worker.mjs', import.meta.url);
  if (workers.controller?.scriptURL === script.href) throw new Error('This browser could not enable cross-origin isolation. Try a current browser with service workers enabled.');
  await workers.register(script.href, { type: 'module', scope: new URL('./', script).pathname, updateViaCache: 'none' });
  await new Promise((resolve, reject) => {
    const controlled = () => {
      if (workers.controller?.scriptURL === script.href) { cleanup(); resolve(); }
    };
    const timer = setTimeout(() => { cleanup(); reject(new Error('Browser setup timed out. Enable service workers and reload.')); }, 10000);
    const cleanup = () => { clearTimeout(timer); workers.removeEventListener('controllerchange', controlled); };
    workers.addEventListener('controllerchange', controlled);
    controlled();
  });
  env.location.reload();
  return false;
}
