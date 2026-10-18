// @ts-check
/** Fatal execution failures require a fresh module, never reuse of its memory. */
export class HostFailure extends Error {
  /**
   * @param {string} reason Failure class shown to the user, such as `trap` or `deadline exceeded`.
   * @param {unknown} error The underlying error or message.
   */
  constructor(reason, error) {
    super(`PDF host ${reason}: ${/** @type {Partial<Error> | null | undefined} */ (error)?.message ?? error}`, {
      cause: error,
    });
    this.name = 'HostFailure';
    this.reason = reason;
  }
}
/**
 * Classify an execution failure from the worker.
 * @param {unknown} error
 */
export function failureReason(error) {
  const message = String(error);
  if (error instanceof RangeError || /memory|allocation|out of bounds/i.test(message)) return 'memory failure';
  if (error instanceof WebAssembly.RuntimeError || /unreachable|trap/i.test(message)) return 'trap';
  return 'execution failure';
}
