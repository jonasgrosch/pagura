// @ts-check
/** Minimal project-owned WASI preview 1 capability boundary (ADR 0005).
 * No preopens, sockets, environment, or arguments. Only fd 0/1/2 exist.
 * Missing imports trap by name; no filesystem/network fallbacks are installed.
 */
export class WasiExit extends Error {
  /** @param {number} code */
  constructor(code) {
    super(`WASI exited with status ${code}`);
    this.code = code;
  }
}

/**
 * WASI preview 1 imports; any other import name returns a function that throws.
 * @typedef {Record<string, (...args: (number | bigint)[]) => number>} WasiImports
 */

/**
 * @param {() => WebAssembly.Memory} getMemory Current memory; views are rebuilt after growth.
 * @param {(destination: Uint8Array) => number} read Fills stdin bytes and returns their count.
 * @param {(fd: number, bytes: Uint8Array) => void} write Receives stdout (1) and stderr (2) bytes.
 * @returns {WasiImports}
 */
export function createWasi(getMemory, read, write) {
  const BADF = 8,
    INVAL = 28;
  const memory = () => getMemory().buffer;
  /**
   * @param {number} pointer
   * @param {number} length
   */
  const region = (pointer, length) => {
    const buffer = memory();
    // WASM i32 imports deliver pointers with the sign bit set as negatives.
    if (!Number.isInteger(pointer) || pointer < -0x80000000 || pointer > 0xffffffff)
      throw new Error('Invalid WASI pointer');
    pointer >>>= 0;
    if (
      !Number.isInteger(pointer) ||
      pointer < 0 ||
      !Number.isInteger(length) ||
      length < 0 ||
      pointer > buffer.byteLength - length
    ) {
      throw new Error('WASI memory access outside linear memory');
    }
    return new Uint8Array(buffer, pointer, length);
  };
  /**
   * @param {number} pointer
   * @param {number} size
   */
  const view = (pointer, size) => {
    const bytes = region(pointer, size);
    return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  };
  /**
   * @param {number} pointer
   * @param {number} value
   */
  const put32 = (pointer, value) => view(pointer, 4).setUint32(0, value, true);
  /**
   * @param {number} count
   * @param {number} size
   */
  const emptySizes = (count, size) => {
    put32(count, 0);
    put32(size, 0);
    return 0;
  };
  const imports = {
    args_sizes_get: emptySizes,
    args_get: () => 0,
    environ_sizes_get: emptySizes,
    environ_get: () => 0,
    fd_prestat_get: () => BADF,
    /**
     * @param {number} fd
     * @param {number} iovs
     * @param {number} count
     * @param {number} result
     */
    fd_read(fd, iovs, count, result) {
      if (fd !== 0) return BADF;
      count >>>= 0;
      const vectors = view(iovs, count * 8);
      region(result, 4);
      let total = 0;
      for (let i = 0; i < count; i++) {
        const buffer = region(vectors.getUint32(i * 8, true), vectors.getUint32(i * 8 + 4, true));
        if (!buffer.length) continue;
        total = read(buffer);
        break; // legal short read, one mailbox segment
      }
      put32(result, total);
      return 0;
    },
    /**
     * @param {number} fd
     * @param {number} iovs
     * @param {number} count
     * @param {number} result
     */
    fd_write(fd, iovs, count, result) {
      if (fd !== 1 && fd !== 2) return BADF;
      count >>>= 0;
      const vectors = view(iovs, count * 8);
      region(result, 4);
      let total = 0;
      for (let i = 0; i < count; i++) {
        const bytes = region(vectors.getUint32(i * 8, true), vectors.getUint32(i * 8 + 4, true));
        if (bytes.length > 0xffffffff - total) return INVAL;
        write(fd, bytes);
        total += bytes.length;
      }
      put32(result, total);
      return 0;
    },
    /**
     * @param {number} clock
     * @param {bigint} _precision
     * @param {number} result
     */
    clock_time_get(clock, _precision, result) {
      if (clock !== 0 && clock !== 1) return INVAL;
      const nanos = clock === 0 ? BigInt(Date.now()) * 1000000n : BigInt(Math.floor(performance.now() * 1000000));
      view(result, 8).setBigUint64(0, nanos, true);
      return 0;
    },
    /**
     * @param {number} pointer
     * @param {number} length
     */
    random_get(pointer, length) {
      length >>>= 0;
      const bytes = region(pointer, length);
      for (let offset = 0; offset < length; offset += 65536)
        crypto.getRandomValues(bytes.subarray(offset, offset + 65536));
      return 0;
    },
    /** @param {number} code */
    proc_exit(code) {
      throw new WasiExit(code);
    },
  };
  return /** @type {WasiImports} */ (
    new Proxy(imports, {
      get(target, name) {
        if (Object.hasOwn(target, name)) return /** @type {Record<string | symbol, unknown>} */ (target)[name];
        return () => {
          throw new Error(`Unsupported WASI import: wasi_snapshot_preview1.${String(name)}`);
        };
      },
    })
  );
}
