/** One bounded 64 KiB stdin mailbox. Ownership transfers via atomic state:
 * 0 empty, 1 ready, 2 EOF. Only the worker blocks; the UI never Atomics.waits.
 */
export const INPUT_BYTES = 64 * 1024;
export function createInput() { return new SharedArrayBuffer(8 + INPUT_BYTES); }

/** Main-thread producer. Requests can span arbitrarily many mailbox chunks. */
export class InputWriter {
  constructor(shared) {
    this.control = new Int32Array(shared, 0, 2); this.data = new Uint8Array(shared, 8);
    this.queue = []; this.offset = 0; this.ended = false;
  }
  write(bytes) {
    if (this.ended) throw new Error('Host input is closed');
    this.queue.push(bytes); this.pump();
  }
  pump() {
    if (Atomics.load(this.control, 0) !== 0) return;
    const bytes = this.queue[0];
    if (!bytes) {
      if (this.ended) { Atomics.store(this.control, 0, 2); Atomics.notify(this.control, 0); }
      return;
    }
    const count = Math.min(bytes.length - this.offset, this.data.length);
    this.data.set(bytes.subarray(this.offset, this.offset + count));
    this.offset += count;
    if (this.offset === bytes.length) { this.queue.shift(); this.offset = 0; }
    Atomics.store(this.control, 1, count); Atomics.store(this.control, 0, 1); Atomics.notify(this.control, 0);
  }
  end() { this.ended = true; this.pump(); }
}

/** Worker consumer; a short read is legal and avoids waiting to fill iovecs. */
export function inputReader(shared, drained) {
  const control = new Int32Array(shared, 0, 2), data = new Uint8Array(shared, 8);
  let offset = 0;
  return (destination) => {
    if (!destination.length) return 0;
    let state;
    while ((state = Atomics.load(control, 0)) === 0) Atomics.wait(control, 0, 0);
    if (state === 2) return 0;
    const length = Atomics.load(control, 1);
    if (state !== 1 || length <= offset || length > data.length) throw new Error('Invalid stdin mailbox state');
    const count = Math.min(destination.length, length - offset);
    destination.set(data.subarray(offset, offset + count)); offset += count;
    if (offset === length) { offset = 0; Atomics.store(control, 0, 0); drained(); }
    return count;
  };
}
