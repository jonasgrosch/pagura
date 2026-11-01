/** Versioned prepared-geometry packet. No per-pixel CPU coverage generation. */
export function validateRaster(bytes, limits) {
  if (!(bytes instanceof Uint8Array) || bytes.length < 96 || bytes.length % 4 || bytes.length > 64 * 1024 * 1024) throw new Error('Invalid GPU geometry size');
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const words = new Uint32Array(bytes.length / 4);
  for (let i = 0; i < words.length; i++) words[i] = view.getUint32(i * 4, true);
  const [magic, version, width, height, columns, bins, x0, y0, marksAt, edgesAt, extrasAt, binsAt, refsAt, marks, edges, extras, refs, total] = words;
  const shared = version === 3, stride = shared ? 20 : 16;
  if (magic !== 0x32555047 || ![2, 3].includes(version) || !width || !height || width * height > 16 * 1024 * 1024 || total !== words.length
      || columns !== Math.ceil(width / 32) || bins !== columns * Math.ceil(height / 8)
      || marksAt !== 24 || edgesAt !== marksAt + marks * stride || extrasAt !== edgesAt + edges * 8
      || binsAt !== extrasAt + extras * 4 + (shared ? words[23] : 0) || refsAt !== binsAt + bins * 2 || total !== refsAt + refs
      || x0 + width > 0xffffffff || y0 + height > 0xffffffff || words[19] > 1) throw new Error('Invalid GPU geometry header');
  if (limits && (bytes.length > limits.maxStorageBufferBindingSize || width * height * 4 > limits.maxStorageBufferBindingSize
      || Math.ceil(width / 64) > limits.maxComputeWorkgroupsPerDimension || height > limits.maxComputeWorkgroupsPerDimension)) throw new Error('GPU geometry exceeds device limits');
  if (shared) {
    if (words[22] !== extrasAt + extras * 4) throw new Error('Invalid shared edge index');
    for (let i = 0; i < words[23]; i++) if (words[words[22] + i] >= edges) throw new Error('Invalid shared edge reference');
  }
  for (let i = 0; i < edges; i++) {
    const at = edgesAt + i * 8;
    if (words[at] > 1) throw new Error('Unknown GPU edge');
    for (let j = 1; j <= 6; j++) if (!Number.isFinite(view.getFloat32((at + j) * 4, true))) throw new Error('Nonfinite GPU edge');
  }
  let previous = 0, work = 0;
  for (let b = 0; b < bins; b++) {
    const start = words[binsAt + b * 2], end = words[binsAt + b * 2 + 1];
    if (start !== previous || end < start || end > refs) throw new Error('Invalid GPU geometry bin');
    let last = -1;
    for (let i = start; i < end; i++) {
      const id = words[refsAt + i];
      if (id >= marks || id <= last) throw new Error('GPU geometry source order');
      last = id;const at = marksAt + id * stride;
      work += (words[at + 5] - words[at + 4] + 1) * 256;
    }
    previous = end;
  }
  if (previous !== refs || work > 2e9) throw new Error('GPU geometry work budget exceeded');
  for (let i = 0; i < marks; i++) {
    const at = marksAt + i * stride;
    if (words[at] < x0 || words[at + 1] < y0 || words[at + 2] > x0 + width || words[at + 3] > y0 + height
        || words[at] >= words[at + 2] || words[at + 1] >= words[at + 3] || words[at + 4] > words[at + 5] || words[at + 5] > (shared ? words[23] : edges)
        || words[at + 6] > 1 || words[at + 7] > 1) throw new Error('Invalid GPU geometry mark');
    if (shared && (!Number.isFinite(view.getFloat32((at + 16) * 4, true)) || !Number.isFinite(view.getFloat32((at + 17) * 4, true)) || words[at + 18] > 1)) throw new Error('Invalid GPU placement');
    for (const offset of [12, 14]) {
      const a = words[at + offset], end = words[at + offset + 1];
      if (a > end || end > extras) throw new Error('Invalid GPU correction range');
      let lastY = -1, lastX = -1;
      for (let e = a; e < end; e++) {
        const p = extrasAt + e * 4, [y, x, right, c] = words.subarray(p, p + 4);
        if (x < words[at] || right > words[at + 2] || x >= right || y < words[at + 1] || y >= words[at + 3] || c > 255
            || y < lastY || y === lastY && x < lastX) throw new Error('Invalid GPU correction ordering');
        lastY = y;lastX = right;
      }
    }
  }
  const unique = new Set();let instanced = 0;
  if (shared) for (let i=0;i<marks;i++) {const at=marksAt+i*stride;if(words[at+18]){instanced++;unique.add(`${words[at+4]}:${words[at+5]}`);}}
  return {shared, edge_references: shared ? words[23] : edges, shared_sources: unique.size, instanced_marks: instanced, words, width, height, spans: marks, references: refs, marks, edges, correction_cells: words[20], clip_runs: words[21], local: !!words[19]};
}
