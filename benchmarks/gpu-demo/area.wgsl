fn q_at(a: vec2f, b: vec2f, c: vec2f, t: f32) -> vec2f {
  return mix(mix(a, b, t), mix(b, c, t), t);
}
fn parameter(p: f32, q: f32, r: f32, value: f32) -> f32 {
  if (p == r) { return 0.0; }
  let s = sign(r - p);
  let delta = clamp((value - p) * s, 0.0, abs(r - p));
  if (delta == 0.0) { return 0.0; }
  if (delta == abs(r - p)) { return 1.0; }
  let linear = 2.0 * (q - p) * s;
  let square = ((r - q) - (q - p)) * s;
  return clamp(2.0 * delta / (linear + sqrt(max(0.0, linear * linear + 4.0 * square * delta))), 0.0, 1.0);
}
fn line_area(a: vec2f, b: vec2f) -> f32 {
  if (a.y == b.y) { return 0.0; }
  let down = a.y < b.y;
  let top = select(b, a, down); let bottom = select(a, b, down);
  let lo = max(0.0, top.y); let hi = min(1.0, bottom.y);
  if (lo >= hi) { return 0.0; }
  let xa = mix(top.x, bottom.x, clamp((lo - top.y) / (bottom.y - top.y), 0.0, 1.0));
  let xb = mix(top.x, bottom.x, clamp((hi - top.y) / (bottom.y - top.y), 0.0, 1.0));
  var area: f32;
  if (xa == xb) { area = clamp(xa, 0.0, 1.0); }
  else {
    let left = clamp(-xa / (xb - xa), 0.0, 1.0);
    let right = clamp((1.0 - xa) / (xb - xa), 0.0, 1.0);
    let start = min(left, right); let end = max(left, right);
    let full = select(right, 1.0 - right, xa < xb);
    area = full + (end - start) * (mix(xa, xb, start) + mix(xa, xb, end)) * 0.5;
  }
  return select(-1.0, 1.0, down) * (hi - lo) * area;
}
fn quadratic_area(a: vec2f, b: vec2f, c: vec2f) -> f32 {
  if (min(a.y, c.y) >= 1.0 || max(a.y, c.y) <= 0.0 || a.y == c.y) { return 0.0; }
  let t0 = parameter(a.y, b.y, c.y, 0.0); let t1 = parameter(a.y, b.y, c.y, 1.0);
  let lo = min(t0, t1); let hi = max(t0, t1);
  let p = q_at(a, b, c, lo); let q = q_at(a, b, c, hi);
  if (p.x == q.x) { return (q.y - p.y) * clamp(p.x, 0.0, 1.0); }
  let left = clamp(parameter(a.x, b.x, c.x, 0.0), lo, hi);
  let right = clamp(parameter(a.x, b.x, c.x, 1.0), lo, hi);
  let start = min(left, right); let end = max(left, right);
  let full = select(q_at(a, b, c, right).y - p.y, q.y - q_at(a, b, c, right).y, p.x < q.x);
  let pa = q_at(a, b, c, start);
  let pb = pa + mix(b - a, c - b, start) * (end - start);
  let pc = q_at(a, b, c, end);
  let v = 2.0 * (pb - pa); let acc = (pc - pb) - (pb - pa);
  let integral = pa.x * (pc.y - pa.y) + v.x * (v.y * 0.5 + acc.y * (2.0 / 3.0)) + acc.x * (v.y / 3.0 + acc.y * 0.5);
  return full + integral;
}
