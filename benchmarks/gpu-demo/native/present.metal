vertex float4 presentVertex(uint i [[vertex_id]]) {
  auto x=float((i<<1u)&2u),y=float(i&2u);return float4(x*2.0f-1.0f,y*2.0f-1.0f,0.0f,1.0f);
}
fragment float4 presentFragment(float4 p [[position]],device const uint* rgba [[buffer(0)]],constant uint4& view [[buffer(1)]],constant uint& marker [[buffer(2)]]) {
  if(uint(p.y)>=480u)return (marker&1u)?float4(0.0f,0.8f,0.4f,1.0f):float4(0.6f,0.0f,0.7f,1.0f);
  auto x=uint(p.x)+view.z,y=uint(p.y)+view.w;
  if(x>=view.x||y>=view.y)return float4(1.0f);
  auto color=rgba[y*view.x+x];return float4(float(color&255u),float((color>>8u)&255u),float((color>>16u)&255u),float(color>>24u))/255.0f;
}
