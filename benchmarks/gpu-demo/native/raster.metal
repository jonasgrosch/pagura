// Native counterpart of raster.wgsl; identical packet and area formulas.
#include <metal_stdlib>
using namespace metal;
float2 point(device const uint* data, uint at) { return float2(as_type<float>(data[at]), as_type<float>(data[at + 1u])); }
// Last run starting at or before the pixel, found in sorted (y,x) order.
uint lookup(device const uint* data, uint start, uint end, uint x, uint y) {
  auto lo = start; auto hi = end;
  while (lo < hi) {
    auto mid = lo + (hi - lo) / 2u; auto at = data[10] + mid * 4u;
    if (data[at] < y || (data[at] == y && data[at + 1u] <= x)) { lo = mid + 1u; } else { hi = mid; }
  }
  if (lo == start) { return 0xffffffffu; }
  auto at = data[10] + (lo - 1u) * 4u;
  if (data[at] == y && data[at + 1u] <= x && x < data[at + 2u]) { return data[at + 3u]; }
  return 0xffffffffu;
}
uint coverage(device const uint* data, uint mark, uint x, uint y) {
  auto fixed = lookup(data, data[mark + 12u], data[mark + 13u], x, y);
  if (fixed != 0xffffffffu) { return fixed; }
  auto pixel = float2(float(x - data[mark + 8u]), float(y - data[mark + 9u]));
  if (data[1] == 3u) { pixel -= point(data, mark + 16u); }
  auto sum = 0.0f; auto correction = 0.0f;
  for (auto i = data[mark + 4u]; i < data[mark + 5u]; i++) {
    auto edge = i;
    if (data[1] == 3u) { edge = data[data[22] + i]; }
    auto at = data[9] + edge * 8u;
    auto a = point(data, at + 1u) - pixel; auto b = point(data, at + 3u) - pixel; auto c = point(data, at + 5u) - pixel;
    float area;
    if (data[at] == 0u) { area = line_area(a, b); } else { area = quadratic_area(a, b, c); }
    auto adjusted = area - correction; auto next = sum + adjusted;
    correction = (next - sum) - adjusted; sum = next;
  }
  auto area = abs(sum);
  if (data[mark + 7u] != 0u) { auto t = fmod(area, 2.0f); area = min(t, 2.0f - t); }
  auto value = uint(clamp(area, 0.0f, 1.0f) * 255.0f + 0.5f);
  if (data[mark + 6u] != 0u) {
    auto clip = lookup(data, data[mark + 14u], data[mark + 15u], x, y);
    if (clip == 0xffffffffu) { value = 0u; }
    else if (clip != 255u) { value = (value == 255u ? clip : 0u); }
  }
  return value;
}
kernel void rasterize(device const uint* data [[buffer(0)]], device uint* rgba [[buffer(1)]], uint3 id [[thread_position_in_grid]]) {
  auto width = data[2]; auto height = data[3];
  if (id.x >= width || id.y >= height) { return; }
  auto x = id.x + data[6]; auto y = id.y + data[7];
  auto tile = (id.y / 8u) * data[4] + id.x / 32u;
  auto at = data[11] + tile * 2u;
  auto background = data[18];
  auto color = uint3(background & 255u, (background >> 8u) & 255u, (background >> 16u) & 255u) * 257u;
  for (auto i = data[at]; i < data[at + 1u]; i++) {
    auto mark = data[8] + data[data[12] + i] * (data[1] == 3u ? 20u : 16u);
    if (x < data[mark] || y < data[mark + 1u] || x >= data[mark + 2u] || y >= data[mark + 3u]) { continue; }
    auto c = coverage(data, mark, x, y);
    auto rg = data[mark + 10u]; auto ba = data[mark + 11u];
    auto source = uint3(rg & 65535u, rg >> 16u, ba & 65535u);
    color = (c * source + (255u - c) * color + 127u) / 255u;
  }
  auto bytes = (color + 128u) / 257u;
  rgba[id.y * width + id.x] = bytes.x | (bytes.y << 8u) | (bytes.z << 16u) | 0xff000000u;
}
