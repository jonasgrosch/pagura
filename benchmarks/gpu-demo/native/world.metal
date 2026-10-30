// The same source-local integration and sparse per-view patches as world.wgsl.
uint worldPatch(device const uint* view,uint start,uint end,uint x,uint y){
  auto lo=start,hi=end;
  while(lo<hi){auto mid=lo+(hi-lo)/2u;auto at=view[4]+mid*4u;
    if(view[at]<y||(view[at]==y&&view[at+1u]<=x))lo=mid+1u;else hi=mid;}
  if(lo==start)return 0xffffffffu;auto at=view[4]+(lo-1u)*4u;
  if(view[at]==y&&view[at+1u]<=x&&x<view[at+2u])return view[at+3u];return 0xffffffffu;
}
uint worldCoverage(device const uint* data,device const uint* view,uint mark,uint v,uint x,uint y){
  auto fixed=worldPatch(view,view[v+8u],view[v+9u],x,y);if(fixed!=0xffffffffu)return fixed;
  auto scale=as_type<float>(view[2]);
  auto pixel=float2(float(int(x)-int(view[v+4u])),float(int(y)-int(view[v+5u])))-point(view,v+6u);
  auto sourceY=(pixel.y+0.5f)/scale;
  auto row=int(floor((sourceY-as_type<float>(data[mark+12u]))/float(data[19])));
  if(row<0||uint(row)>=data[mark+5u])return 0u;
  auto index=data[10]+(data[mark+4u]+uint(row))*2u;
  auto lo=float2(0.0f),hi=float2(1.0f);
  if(data[mark+6u]!=0u){lo=max(lo,point(data,mark+20u)*scale-pixel);hi=min(hi,point(data,mark+22u)*scale-pixel);}
  auto span=hi-lo;if(any(span<=float2(0.0f)))return 0u;
#if WORLD_RECIPROCAL
  auto inverseSpan=1.0f/span;
#endif
  float sum=0.0f,correction=0.0f;
  for(auto i=data[index];i<data[index+1u];i++){
    auto at=data[9]+data[data[20]+i]*8u;
#if WORLD_RECIPROCAL
    auto a=(point(data,at+1u)*scale-pixel-lo)*inverseSpan;
    auto b=(point(data,at+3u)*scale-pixel-lo)*inverseSpan;
    auto c=(point(data,at+5u)*scale-pixel-lo)*inverseSpan;
#else
    auto a=(point(data,at+1u)*scale-pixel-lo)/span;
    auto b=(point(data,at+3u)*scale-pixel-lo)/span;
    auto c=(point(data,at+5u)*scale-pixel-lo)/span;
#endif
    auto area=data[at]==0u?line_area(a,b):quadratic_area(a,b,c);
    auto adjusted=area-correction;auto next=sum+adjusted;correction=(next-sum)-adjusted;sum=next;
  }
  auto area=abs(sum);if(data[mark+7u]!=0u){auto t=fmod(area,2.0f);area=min(t,2.0f-t);}
  return uint(clamp(area,0.0f,1.0f)*span.x*span.y*255.0f+0.5f);
}
kernel void worldRasterize(device const uint* data [[buffer(0)]],device uint* rgba [[buffer(1)]],device const uint* view [[buffer(2)]],uint2 id [[thread_position_in_grid]]){
  if(id.x>=view[0]||id.y>=view[1])return;
  auto scale=as_type<float>(view[2]);auto canonical=(float2(id)+0.5f)/scale;
  auto bx=min(uint(canonical.x)/32u,data[4]-1u),by=min(uint(canonical.y)/8u,data[5]/data[4]-1u);
  auto bin=data[11]+(by*data[4]+bx)*2u;auto background=data[18];
  auto color=uint3(background&255u,(background>>8u)&255u,(background>>16u)&255u)*257u;
  for(auto i=data[bin];i<data[bin+1u];i++){
    auto n=data[data[12]+i],mark=data[8]+n*24u,v=8u+n*12u;
    if(id.x<view[v]||id.y<view[v+1u]||id.x>=view[v+2u]||id.y>=view[v+3u])continue;
    auto c=worldCoverage(data,view,mark,v,id.x,id.y),rg=data[mark+10u],ba=data[mark+11u];
    auto source=uint3(rg&65535u,rg>>16u,ba&65535u);color=(c*source+(255u-c)*color+127u)/255u;
  }
  auto bytes=(color+128u)/257u;rgba[id.y*view[0]+id.x]=bytes.x|(bytes.y<<8u)|(bytes.z<<16u)|0xff000000u;
}
