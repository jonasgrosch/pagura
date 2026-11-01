/** Shared packet indirection and real cache lifecycle checks. */
import {check,oraclePacket,polygonArea} from './coverage.mjs';
import {validateRaster} from './raster.mjs';
import {samples} from './samples.mjs';
import {RetainedSession} from './retained.mjs';

export function sharedPacket(bytes,offsets=[[0,0]]) {
  const old=new Uint32Array(bytes.buffer,bytes.byteOffset,bytes.byteLength/4),count=offsets.length,edges=old[14],bins=old[5];
  const edgesAt=24+count*20,extrasAt=edgesAt+edges*8,indexAt=extrasAt,binsAt=indexAt+edges,refsAt=binsAt+bins*2;
  const words=new Uint32Array(refsAt+bins*count),floats=new Float32Array(words.buffer);
  words.set(old.subarray(0,24));words.set([24,edgesAt,extrasAt,binsAt,refsAt,count,edges,0,bins*count,words.length],8);words[1]=3;words[22]=indexAt;words[23]=edges;
  words.set(old.subarray(old[9],old[10]),edgesAt);
  for(let i=0;i<edges;i++)words[indexAt+i]=i;
  offsets.forEach(([x,y],i)=>{const at=24+i*20;words.set(old.subarray(24,40),at);words[at+4]=0;words[at+5]=edges;floats[at+16]=x;floats[at+17]=y;words[at+18]=1;});
  for(let i=0;i<bins;i++){words.set([i*count,(i+1)*count],binsAt+i*2);for(let j=0;j<count;j++)words[refsAt+i*count+j]=j;}
  return new Uint8Array(words.buffer);
}

async function instancing(gpu) {
  const vertices=[[0.125,0.25],[7.875,1.25],[3.25,7.875]],offsets=[[0.25,0.375],[13.125,4.25]],bytes=sharedPacket(oraclePacket(vertices),offsets);
  const job=gpu.prepare(bytes);let max=0,refused=0;
  try {
    let uninitialized=false;try{await job.render(null);}catch{uninitialized=true;}if(!uninitialized)throw new Error('Uninitialized resident draw succeeded');
    await job.retain();await job.retain();
    const out=await job.render(null);
    for(let y=0;y<16;y++)for(let x=0;x<32;x++){
      let n16=65535;for(const [dx,dy]of offsets){const area=polygonArea(vertices.map(([x,y])=>[x+dx,y+dy]),x,y),c=Math.floor(area*255+0.5+1e-9);n16=Math.floor(((255-c)*n16+127)/255);}
      max=Math.max(max,Math.abs(out.pixels[(y*32+x)*4]-Math.floor((n16+128)/257)));
    }
    if(max>1)throw new Error(`Instanced polygon oracle: ${max}`);
    if(job.counters().uploads!==1)throw new Error('Repeated retain reuploaded geometry');
    for(const mutate of [w=>w[22]=0,w=>w[23]++,w=>w[w[22]]=w[14],w=>w[40]=0x7fc00000,w=>w[42]=2,w=>w[9]++]){
      const bad=bytes.slice();mutate(new Uint32Array(bad.buffer));try{validateRaster(bad);}catch{refused++;}
    }
    if(refused!==6)throw new Error('Malformed shared packet was accepted');
  }finally{job.destroy();}
  let destroyed=false;try{await job.render(null);}catch{destroyed=true;}if(!destroyed)throw new Error('Destroyed job rendered');
  return {pixels:512,max_channel_error:max,malformed_refused:refused,uninitialized_and_destroyed_refused:true};
}

async function lifecycle(engine,gpu) {
  const view=new RetainedSession(engine,gpu);
  try {
    await engine.open(samples[0].make());
    const first=await view.prepare({dpi:40});
    await view.prepare({dpi:50});await view.prepare({dpi:60});const current=await view.prepare({dpi:70});
    if(view.metadata().entries!==3||view.metadata().evictions!==1)throw new Error('GPU three-entry LRU did not evict');
    let evicted=false;try{await first.job.render(null);}catch{evicted=true;}if(!evicted)throw new Error('Evicted job rendered');
    await engine.open(samples[1].make());await view.prepare({dpi:70});
    let invalidated=false;try{await current.job.render(null);}catch{invalidated=true;}if(!invalidated)throw new Error('Old document resource survived');
    await view.clear();if(view.metadata().entries||view.metadata().resident_bytes)throw new Error('GPU clear retained resources');
    return {three_entry_eviction:true,document_invalidation:true,clear_releases:true};
  }finally{await view.clear();}
}
export async function runTests(engine,gpu) {
  const oracle=await instancing(gpu),cache=await lifecycle(engine,gpu);
  const result=await check(engine,gpu,{operation:8,packetTransform:sharedPacket});
  result.shared_oracle=oracle;result.cache_lifecycle=cache;
  return result;
}
