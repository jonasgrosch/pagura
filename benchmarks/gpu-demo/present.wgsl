@group(0) @binding(0) var<storage,read> rgba: array<u32>;
// width, height, viewport x/y; the drawable is a 1:1 viewport.
@group(0) @binding(1) var<uniform> view: vec4u;
@vertex fn triangle(@builtin(vertex_index) i:u32)->@builtin(position) vec4f {
  let x=f32((i<<1u)&2u);let y=f32(i&2u);
  return vec4f(x*2.0-1.0,y*2.0-1.0,0.0,1.0);
}
@fragment fn display(@builtin(position) p:vec4f)->@location(0) vec4f {
  let x=u32(p.x)+view.z;let y=u32(p.y)+view.w;
  if (x>=view.x || y>=view.y) {return vec4f(1.0);}
  return unpack4x8unorm(rgba[y*view.x+x]);
}
