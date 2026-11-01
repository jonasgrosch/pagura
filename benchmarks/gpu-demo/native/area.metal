float2 q_at(float2 a, float2 b, float2 c, float t) {
  return mix(mix(a, b, t), mix(b, c, t), t);
}
float parameter(float p, float q, float r, float value) {
  if (p == r) { return 0.0f; }
  auto s = sign(r - p);
  auto delta = clamp((value - p) * s, 0.0f, abs(r - p));
  if (delta == 0.0f) { return 0.0f; }
  if (delta == abs(r - p)) { return 1.0f; }
  auto linear = 2.0f * (q - p) * s;
  auto square = ((r - q) - (q - p)) * s;
  return clamp(2.0f * delta / (linear + sqrt(max(0.0f, linear * linear + 4.0f * square * delta))), 0.0f, 1.0f);
}
float line_area(float2 a, float2 b) {
  if (a.y == b.y) { return 0.0f; }
  auto down = a.y < b.y;
  auto top = (down ? a : b); auto bottom = (down ? b : a);
  auto lo = max(0.0f, top.y); auto hi = min(1.0f, bottom.y);
  if (lo >= hi) { return 0.0f; }
  auto xa = mix(top.x, bottom.x, clamp((lo - top.y) / (bottom.y - top.y), 0.0f, 1.0f));
  auto xb = mix(top.x, bottom.x, clamp((hi - top.y) / (bottom.y - top.y), 0.0f, 1.0f));
  float area;
  if (xa == xb) { area = clamp(xa, 0.0f, 1.0f); }
  else {
    auto left = clamp(-xa / (xb - xa), 0.0f, 1.0f);
    auto right = clamp((1.0f - xa) / (xb - xa), 0.0f, 1.0f);
    auto start = min(left, right); auto end = max(left, right);
    auto full = (xa < xb ? 1.0f - right : right);
    area = full + (end - start) * (mix(xa, xb, start) + mix(xa, xb, end)) * 0.5f;
  }
  return (down ? 1.0f : -1.0f) * (hi - lo) * area;
}
float quadratic_area(float2 a, float2 b, float2 c) {
  if (min(a.y, c.y) >= 1.0f || max(a.y, c.y) <= 0.0f || a.y == c.y) { return 0.0f; }
  auto t0 = parameter(a.y, b.y, c.y, 0.0f); auto t1 = parameter(a.y, b.y, c.y, 1.0f);
  auto lo = min(t0, t1); auto hi = max(t0, t1);
  auto p = q_at(a, b, c, lo); auto q = q_at(a, b, c, hi);
  if (p.x == q.x) { return (q.y - p.y) * clamp(p.x, 0.0f, 1.0f); }
  auto left = clamp(parameter(a.x, b.x, c.x, 0.0f), lo, hi);
  auto right = clamp(parameter(a.x, b.x, c.x, 1.0f), lo, hi);
  auto start = min(left, right); auto end = max(left, right);
  auto full = (p.x < q.x ? q.y - q_at(a, b, c, right).y : q_at(a, b, c, right).y - p.y);
  auto pa = q_at(a, b, c, start);
  auto pb = pa + mix(b - a, c - b, start) * (end - start);
  auto pc = q_at(a, b, c, end);
  auto v = 2.0f * (pb - pa); auto acc = (pc - pb) - (pb - pa);
  auto integral = pa.x * (pc.y - pa.y) + v.x * (v.y * 0.5f + acc.y * (2.0f / 3.0f)) + acc.x * (v.y / 3.0f + acc.y * 0.5f);
  return full + integral;
}
