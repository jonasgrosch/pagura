// @ts-check
/** @import { Action, GoToAction, Target, UriAction } from './reading_protocol.mjs' */
/**
 * URI data never becomes an executable DOM attribute before confirmation.
 * @param {Pick<UriAction, 'uri' | 'isMap'> & { base?: string | null }} action
 */
export function externalUrl(action) {
  if (action.isMap) throw new Error('Coordinate-appending URI actions are inert in this viewer');
  let url;
  try {
    // Relative URIs require the PDF's Base; never guess from the viewer URL.
    url = action.base === null || action.base === undefined ? new URL(action.uri) : new URL(action.uri, action.base);
  } catch {
    throw new Error('URI cannot be resolved without a valid absolute PDF Base');
  }
  if (!['http:', 'https:', 'mailto:'].includes(url.protocol)) throw new Error(`Blocked URI scheme: ${url.protocol}`);
  if (url.username || url.password) throw new Error('URI credentials are not allowed');
  return url.href;
}
const namedPages = {
  NextPage: 'Go to next page',
  PrevPage: 'Go to previous page',
  FirstPage: 'Go to first page',
  LastPage: 'Go to last page',
};
/**
 * The accessible name of a link or outline target.
 * @param {Target} target
 * @param {(page: number) => string} [pageLabel]
 */
export function targetLabel(target, pageLabel = (page) => String(page + 1)) {
  if (!target.length) return 'Link without a target';
  if (target.length > 1) return 'Multiple actions (not run)';
  const action = target[0];
  if (action.kind === 1) return `Go to page ${pageLabel(action.page)}`;
  if (action.kind === 2) {
    if (action.isMap) return 'Coordinate-appending URI action (not run)';
    try {
      return `Open ${externalUrl(action)} (external; asks before opening)`;
    } catch (error) {
      return /** @type {Error} */ (error).message;
    }
  }
  if (action.kind === 3)
    return Object.hasOwn(namedPages, action.name)
      ? namedPages[/** @type {keyof typeof namedPages} */ (action.name)]
      : `${action.name} named action (not run)`;
  return `${/** @type {Record<number, string | undefined>} */ ({ 4: 'GoToR', 5: 'Launch', 6: 'JavaScript' })[action.kind] ?? /** @type {Partial<Action> & { name?: string }} */ (action).name} action (not run)`;
}
/**
 * @typedef {object} ActivateOptions
 * @property {(action: GoToAction) => unknown} destination Follows an internal destination.
 * @property {(name: string) => unknown} named Runs NextPage, PrevPage, FirstPage or LastPage.
 * @property {(error: unknown) => void} report
 * @property {(message: string) => void} notice Explains an inert target.
 * @property {(message: string) => boolean} [confirm]
 * @property {(url: string) => unknown} [open]
 */
/**
 * Run a target's single supported action; everything else stays inert with a notice.
 * @param {Target} target
 * @param {ActivateOptions} options
 */
export async function activateTarget(
  target,
  {
    destination,
    named,
    report,
    notice,
    confirm = (value) => window.confirm(value),
    open = (value) => window.open(value, '_blank', 'noopener,noreferrer'),
  },
) {
  try {
    if (target.length !== 1) {
      notice(targetLabel(target));
      return;
    }
    const action = target[0];
    if (action.kind === 1) await destination(action);
    else if (action.kind === 2 && !action.isMap) {
      const url = externalUrl(action);
      if (confirm(`Open external link?\n${url}`)) open(url);
    } else if (action.kind === 3 && Object.hasOwn(namedPages, action.name)) await named(action.name);
    else
      notice(
        `${targetLabel(target)}. This viewer does not run scripts, open remote files or perform unsupported actions.`,
      );
  } catch (error) {
    report(error);
  }
}
