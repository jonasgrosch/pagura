/** Independent area oracle, retained lifetimes, wire rejection and fixture parity. */
import {validateWorld,validatePatches,WorldSession} from './world.mjs';
import {createWorldJob} from './world-gpu.mjs';
import {polygonArea} from './coverage.mjs';
import {Presenter,viewport} from './present.mjs';
import {pixelDifference} from './svg.mjs';
import {sha256} from './retained-study.mjs';
import {samples} from './samples.mjs';
const vertices=[[0.125,0.25],[7.875,1.25],[3.25,7.875]],offsets=[[0.25,0.375],[13.125,4.25]];
function oraclePacket(){
  const ma=24,ea=ma+48,ra=ea+24,rra=ra+8,ba=rra+12,bpa=ba+4,w=new Uint32Array(bpa+4),f=new Float32Array(w.buffer);
  w.set([0x34555047,1,32,16,1,2,72,300,ma,ea,ra,ba,bpa,2,3,4,4,w.length,0xffffffff,8,rra,12,1,0]);
  for(let i=0;i<2;i++){
    const at=ma+i*24;w.set([0,0,32,16,0,4,0,0,0,0,0,0xffff0000,0,3,0,0,0,0,i,1,0,0,0,0],at);f[at+12]=-8;f[at+16]=offsets[i][0];f[at+17]=offsets[i][1];
  }
  for(let i=0;i<3;i++){const a=vertices[i],b=vertices[(i+1)%3],at=ea+i*8;w[at]=0;f.set([...a,...b,...b],at+1);}
  for(let i=0;i<4;i++){w.set([i*3,i*3+3],ra+i*2);w.set([0,1,2],rra+i*3);}
  w.set([0,2,2,4],ba);w.set([0,1,0,1],bpa);return new Uint8Array(w.buffer);
}
async function independent(gpu,options){
  const bytes=oraclePacket(),world=validateWorld(bytes),meta={extent_width:32*72/300,extent_height:16*72/300};
  const job=await createWorldJob(gpu,bytes,meta,world,options);const cases=[];
  const canvas=document.createElement('canvas');document.body.append(canvas);const presenter=await Presenter.create(gpu,canvas);
  try {
    for(const dpi of [72,150,173,225,291,300]){
      const out=await job.draw(dpi,{readback:true}),scale=dpi/300,expected=new Uint8Array(out.width*out.height*4);expected.fill(255);
      for(let y=0;y<out.height;y++)for(let x=0;x<out.width;x++){
        let n=65535;for(const [dx,dy]of offsets){const area=polygonArea(vertices.map(([vx,vy])=>[(vx+dx)*scale,(vy+dy)*scale]),x,y),c=Math.floor(area*255+0.5+1e-9);n=Math.floor(((255-c)*n+127)/255);}
        expected.set([Math.floor((n+128)/257),Math.floor((n+128)/257),Math.floor((n+128)/257),255],(y*out.width+x)*4);
      }
      const delta=pixelDifference(out.pixels,expected);if(delta.max_channel_error>1)throw new Error('World source-local independent polygon oracle');cases.push({dpi,...delta});
    }
    const before=job.metadata();presenter.captureNext=true;await job.draw(300,{presenter});const capture=await presenter.capture();
    const pixels=(await job.draw(300,{readback:true})).pixels;
    if(pixelDifference(capture,viewport(pixels,32,16)).max_channel_error)throw new Error('World presentation attachment differs');
    if(before.geometry_uploads!==1||before.entries!==3||before.evictions<3)throw new Error('World bounded view LRU failed');
    let refused=0;
    for(const mutate of [w=>w[0]=0,w=>w[9]++,w=>w[21]++,w=>w[w[20]]=w[14],w=>w[24+18]=w[48+18],w=>w[24+19]=2,w=>w[24+16]=0x7fc00000,w=>w[w[11]]=1,w=>w[w[9]+1]=0x7f7fffff,w=>w[24+11]=0]){
      const bad=bytes.slice();mutate(new Uint32Array(bad.buffer));try{validateWorld(bad);}catch{refused++;}
    }
    if(refused!==10)throw new Error('Malformed world packet accepted');
    return {cases,malformed_refused:refused,geometry_uploads:before.geometry_uploads,evictions:before.evictions,presentation_identical:true};
  }finally{job.destroy();presenter.destroy();canvas.remove();}
}
export async function runTests(engine,gpu,options={}){
  const oracle=await independent(gpu,options),failures=[],cases=[],unsupported={};
  const index=await(await fetch('/fixture-index.json')).json();let admitted=0,variants=0;
  for(const fixture of index){
    const world=new WorldSession(engine,gpu,options),request={page:0,dpi:fixture.dpi??72};
    try {
      await engine.open(new Uint8Array(await(await fetch(fixture.path)).arrayBuffer()));
      const cpu=await engine.render(2,request),cached=await engine.render(7,request);
      if(await sha256(cpu.data)!==await sha256(cached.data))throw new Error('CPU prepared scene changed');
      const out=await world.draw(request,{readback:true});
      const result={id:fixture.id,cpu_sha256:await sha256(cpu.data)};
      if(out.fallback){result.fallback=out.fallback;unsupported[out.fallback]=(unsupported[out.fallback]??0)+1;}
      else {
        admitted++;result.accuracy=pixelDifference(out.pixels,cpu.data);
        if(result.accuracy.max_channel_error>2)throw new Error('World fixture accuracy exceeded two levels');
        const packet=await engine.render(11,request),patches=await engine.render(12,request);
        result.world_sha256=await sha256(packet.data);result.patch_sha256=await sha256(patches.data);
        validatePatches(patches.data);
        if(admitted<=80){
          const repeat=await world.draw(request,{readback:true});if(pixelDifference(repeat.pixels,out.pixels).max_channel_error)throw new Error('World repeat output changed');
          const before=world.job.metadata();result.zoom_accuracy=[];
          for(const dpi of [151,173,225]){const c=await engine.render(2,{dpi}),view=await world.draw({dpi},{readback:true});if(view.fallback)break;variants++;if(c.data.length!==view.pixels.length)throw new Error('World zoom dimensions');const delta=pixelDifference(view.pixels,c.data);result.zoom_accuracy.push({dpi,...delta});if(delta.max_channel_error>2)throw new Error('World fixture zoom accuracy exceeded two levels');}
          if(world.job.metadata().geometry_uploads!==before.geometry_uploads)throw new Error('World zoom reuploaded geometry');
        }
      }
      cases.push(result);
    }catch(error){failures.push({id:fixture.id,error:String(error)});}
    finally{await world.clear();}
    if(cases.length%150===0)console.log('World fixture decisions',cases.length,index.length);
  }
  await engine.open(samples[0].make());const first=await engine.render(11,{dpi:72}),second=await engine.render(11,{dpi:299});
  if(first.meta.fallback||second.meta.fallback||await sha256(first.data)!==await sha256(second.data)||!second.meta.world_cache_hit)failures.push({error:'Canonical vector export is DPI dependent'});
  return {fixtures:index.length,admitted,fallback:index.length-admitted,variants,oracle,cases,unsupported,failures};
}

export async function runUi(demo) {
  const wait=async condition=>{const deadline=performance.now()+120000;while(!condition()){if(performance.now()>deadline)throw new Error('Display UI timeout');await new Promise(resolve=>setTimeout(resolve,25));}};
  document.querySelector('#controls').requestSubmit();await wait(()=>!demo.busy&&demo.current);
  document.querySelector('#display-run').click();await wait(()=>!demo.busy);
  if(!demo.lastDisplay?.modes.world_direct)throw new Error(document.querySelector('#display-status').textContent);
  const slider=document.querySelector('#display-dpi');slider.value='173';slider.dispatchEvent(new Event('input'));await wait(()=>!demo.busy);
  if(document.querySelector('#display-target').textContent!=='173 DPI')throw new Error('Live world view did not update');
  document.querySelector('#display-zoom').click();await wait(()=>!demo.busy);
  if(demo.lastDisplayZoom?.novel.world_direct.samples.length!==12)throw new Error('Novel zoom controls did not complete');
  if(!document.querySelector('#display-gpu-caption').textContent.includes('World geometry'))throw new Error('World preview unavailable');
  return {failures:[],direct_modes:Object.keys(demo.lastDisplay.modes),live_dpi:173,novel_targets:12,warm_targets:3};
}
