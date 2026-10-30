// Experimental pixel-area integration of the existing prepared geometry.
// Coordinates are either absolute f32 or rebased in f64 before narrowing.
// Exact exceptional winding/joint-clip cells come from CPU preparation.
@group(0) @binding(0) var<storage, read> data: array<u32>;
@group(0) @binding(1) var<storage, read_write> rgba: array<u32>;

fn point(at: u32) -> vec2f { return vec2f(bitcast<f32>(data[at]), bitcast<f32>(data[at + 1u])); }
// Last run starting at or before the pixel, found in sorted (y,x) order.
fn lookup(start: u32, end: u32, x: u32, y: u32) -> u32 {
  var lo = start; var hi = end;
  while (lo < hi) {
    let mid = lo + (hi - lo) / 2u; let at = data[10] + mid * 4u;
    if (data[at] < y || (data[at] == y && data[at + 1u] <= x)) { lo = mid + 1u; } else { hi = mid; }
  }
  if (lo == start) { return 0xffffffffu; }
  let at = data[10] + (lo - 1u) * 4u;
  if (data[at] == y && data[at + 1u] <= x && x < data[at + 2u]) { return data[at + 3u]; }
  return 0xffffffffu;
}
fn coverage(mark: u32, x: u32, y: u32) -> u32 {
  let fixed = lookup(data[mark + 12u], data[mark + 13u], x, y);
  if (fixed != 0xffffffffu) { return fixed; }
  var pixel = vec2f(f32(x - data[mark + 8u]), f32(y - data[mark + 9u]));
  if (data[1] == 3u) { pixel -= point(mark + 16u); }
  var sum = 0.0; var correction = 0.0;
  for (var i = data[mark + 4u]; i < data[mark + 5u]; i++) {
    var edge = i;
    if (data[1] == 3u) { edge = data[data[22] + i]; }
    let at = data[9] + edge * 8u;
    let a = point(at + 1u) - pixel; let b = point(at + 3u) - pixel; let c = point(at + 5u) - pixel;
    var area: f32;
    if (data[at] == 0u) { area = line_area(a, b); } else { area = quadratic_area(a, b, c); }
    let adjusted = area - correction; let next = sum + adjusted;
    correction = (next - sum) - adjusted; sum = next;
  }
  var area = abs(sum);
  if (data[mark + 7u] != 0u) { let t = area % 2.0; area = min(t, 2.0 - t); }
  var value = u32(clamp(area, 0.0, 1.0) * 255.0 + 0.5);
  if (data[mark + 6u] != 0u) {
    let clip = lookup(data[mark + 14u], data[mark + 15u], x, y);
    if (clip == 0xffffffffu) { value = 0u; }
    else if (clip != 255u) { value = select(0u, clip, value == 255u); }
  }
  return value;
}
@compute @workgroup_size(64)
fn rasterize(@builtin(global_invocation_id) id: vec3u) {
  let width = data[2]; let height = data[3];
  if (id.x >= width || id.y >= height) { return; }
  let x = id.x + data[6]; let y = id.y + data[7];
  let tile = (id.y / 8u) * data[4] + id.x / 32u;
  let at = data[11] + tile * 2u;
  let background = data[18];
  var color = vec3u(background & 255u, (background >> 8u) & 255u, (background >> 16u) & 255u) * 257u;
  for (var i = data[at]; i < data[at + 1u]; i++) {
    let mark = data[8] + data[data[12] + i] * select(16u, 20u, data[1] == 3u);
    if (x < data[mark] || y < data[mark + 1u] || x >= data[mark + 2u] || y >= data[mark + 3u]) { continue; }
    let c = coverage(mark, x, y);
    let rg = data[mark + 10u]; let ba = data[mark + 11u];
    let source = vec3u(rg & 65535u, rg >> 16u, ba & 65535u);
    color = (c * source + (255u - c) * color + 127u) / 255u;
  }
  let bytes = (color + 128u) / 257u;
  rgba[id.y * width + id.x] = bytes.x | (bytes.y << 8u) | (bytes.z << 16u) | 0xff000000u;
}
