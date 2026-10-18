/** URI data never becomes an executable DOM attribute before confirmation. */
export function externalUrl(action) {
  if (action.isMap) throw new Error('Coordinate-appending URI actions are inert in this viewer');
  let url;
  try {
    // Relative URIs require the PDF's Base; never guess from the viewer URL.
    url = action.base === null || action.base === undefined ? new URL(action.uri) : new URL(action.uri, action.base);
  } catch { throw new Error('URI cannot be resolved without a valid absolute PDF Base'); }
  if (!['http:', 'https:', 'mailto:'].includes(url.protocol)) throw new Error(`Blocked URI scheme: ${url.protocol}`);
  if (url.username || url.password) throw new Error('URI credentials are not allowed');
  return url.href;
}
export function targetLabel(target) {
  if (!target.length) return 'Link without a target';
  if (target.length > 1) return 'Multiple actions: inert in this viewer';
  const action = target[0];
  if (action.kind === 1) return `Go to page ${action.page + 1}`;
  if (action.kind === 2) { try { return externalUrl(action); } catch (error) { return String(error); } }
  if (action.kind === 3) return ['NextPage','PrevPage','FirstPage','LastPage'].includes(action.name) ? action.name : `${action.name}: inert; unsupported named action`;
  return `${({ 4: 'GoToR', 5: 'Launch', 6: 'JavaScript' })[action.kind] ?? action.name}: inert; this viewer never opens files or runs PDF scripts`;
}
export async function activateTarget(target, { destination, named, report, confirm = (value) => window.confirm(value), open = (value) => window.open(value, '_blank', 'noopener,noreferrer') }) {
  try {
    if (target.length !== 1) throw new Error(targetLabel(target));
    const action = target[0];
    if (action.kind === 1) await destination(action);
    else if (action.kind === 2) {
      const url = externalUrl(action);
      if (confirm(`Open external link?\n${url}`)) open(url);
    } else if (action.kind === 3 && ['NextPage', 'PrevPage', 'FirstPage', 'LastPage'].includes(action.name)) await named(action.name);
    else throw new Error(targetLabel(target));
  } catch (error) { report(error); }
}
