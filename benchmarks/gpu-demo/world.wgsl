// Immutable canonical vectors; only view-local transforms change with zoom.
override hoist_clip_reciprocal:bool=false;
@group(0) @binding(0) var<storage,read> data:array<u32>;
@group(0) @binding(1) var<storage,read_write> rgba:array<u32>;
@group(0) @binding(2) var<storage,read> view:array<u32>;
fn point(at:u32)->vec2f{return vec2f(bitcast<f32>(data[at]),bitcast<f32>(data[at+1u]));}
fn vp(at:u32)->vec2f{return vec2f(bitcast<f32>(view[at]),bitcast<f32>(view[at+1u]));}
fn patch_value(start:u32,end:u32,x:u32,y:u32)->u32 {
  var lo=start;var hi=end;
  while(lo<hi){let mid=lo+(hi-lo)/2u;let at=view[4]+mid*4u;
    if(view[at]<y || (view[at]==y && view[at+1u]<=x)){lo=mid+1u;}else{hi=mid;}}
  if(lo==start){return 0xffffffffu;}let at=view[4]+(lo-1u)*4u;
  if(view[at]==y && view[at+1u]<=x && x<view[at+2u]){return view[at+3u];}return 0xffffffffu;
}
fn coverage(mark:u32,v:u32,x:u32,y:u32)->u32 {
  let fixed=patch_value(view[v+8u],view[v+9u],x,y);if(fixed!=0xffffffffu){return fixed;}
  let scale=bitcast<f32>(view[2]);
  let pixel=vec2f(f32(i32(x)-i32(view[v+4u])),f32(i32(y)-i32(view[v+5u])))-vp(v+6u);
  let sourceY=(pixel.y+0.5)/scale;
  let row=i32(floor((sourceY-bitcast<f32>(data[mark+12u]))/f32(data[19])));
  if(row<0 || u32(row)>=data[mark+5u]){return 0u;}
  let index=data[10]+(data[mark+4u]+u32(row))*2u;
  var lo=vec2f(0.0);var hi=vec2f(1.0);
  if(data[mark+6u]!=0u){lo=max(lo,point(mark+20u)*scale-pixel);hi=min(hi,point(mark+22u)*scale-pixel);}
  let span=hi-lo;if(any(span<=vec2f(0.0))){return 0u;}
  let inverseSpan=vec2f(1.0)/span;
  var sum=0.0;var correction=0.0;
  for(var i=data[index];i<data[index+1u];i++){
    let at=data[9]+data[data[20]+i]*8u;
    var a:vec2f;var b:vec2f;var c:vec2f;
    if(hoist_clip_reciprocal){
      a=(point(at+1u)*scale-pixel-lo)*inverseSpan;
      b=(point(at+3u)*scale-pixel-lo)*inverseSpan;
      c=(point(at+5u)*scale-pixel-lo)*inverseSpan;
    }else{
      a=(point(at+1u)*scale-pixel-lo)/span;
      b=(point(at+3u)*scale-pixel-lo)/span;
      c=(point(at+5u)*scale-pixel-lo)/span;
    }
    var area:f32;if(data[at]==0u){area=line_area(a,b);}else{area=quadratic_area(a,b,c);}
    let adjusted=area-correction;let next=sum+adjusted;correction=(next-sum)-adjusted;sum=next;
  }
  var area=abs(sum);if(data[mark+7u]!=0u){let t=area%2.0;area=min(t,2.0-t);}
  return u32(clamp(area,0.0,1.0)*span.x*span.y*255.0+0.5);
}
@compute @workgroup_size(64)
fn rasterize(@builtin(global_invocation_id) id:vec3u){
  if(id.x>=view[0] || id.y>=view[1]){return;}
  let scale=bitcast<f32>(view[2]);
  let canonical=(vec2f(id.xy)+0.5)/scale;
  let bx=min(u32(canonical.x)/32u,data[4]-1u);
  let by=min(u32(canonical.y)/8u,data[5]/data[4]-1u);
  let bin=data[11]+(by*data[4]+bx)*2u;
  let background=data[18];var color=vec3u(background&255u,(background>>8u)&255u,(background>>16u)&255u)*257u;
  for(var i=data[bin];i<data[bin+1u];i++){
    let n=data[data[12]+i];let mark=data[8]+n*24u;let v=8u+n*12u;
    if(id.x<view[v] || id.y<view[v+1u] || id.x>=view[v+2u] || id.y>=view[v+3u]){continue;}
    let c=coverage(mark,v,id.x,id.y);let rg=data[mark+10u];let ba=data[mark+11u];
    let source=vec3u(rg&65535u,rg>>16u,ba&65535u);
    color=(c*source+(255u-c)*color+127u)/255u;
  }
  let bytes=(color+128u)/257u;rgba[id.y*view[0]+id.x]=bytes.x|(bytes.y<<8u)|(bytes.z<<16u)|0xff000000u;
}
