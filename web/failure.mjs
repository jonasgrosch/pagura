/** Fatal execution failures require a fresh module, never reuse of its memory. */
export class HostFailure extends Error {
  constructor(reason, error) {
    super(`PDF host ${reason}: ${error?.message ?? error}`, { cause: error });
    this.name = 'HostFailure';
    this.reason = reason;
  }
}
export function failureReason(error) {
  const message = String(error);
  if (error instanceof RangeError || /memory|allocation|out of bounds/i.test(message)) return 'memory failure';
  if (error instanceof WebAssembly.RuntimeError || /unreachable|trap/i.test(message)) return 'trap';
  return 'execution failure';
}
