/** Same retained experiment in browser and native hosts. Diagnostics untimed. */
import {RetainedSession} from './retained.mjs';
import {stats, assertPixels} from './measure.mjs';
import {pixelDifference} from './svg.mjs';
export const sha256 = async bytes => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), b=>b.toString(16).padStart(2,'0')).join('');
const equal = (a,b) => {if(a.length!==b.length)return false;for(let i=0;i<a.length;i++)if(a[i]!==b[i])return false;return true;};
const summarize = rows => Object.fromEntries(['render_ms','frame_ms','ready_ms','kernel_ms'].filter(k=>rows[0]?.[k]!=null).map(k=>[k,stats(rows.map(r=>r[k]))]));

export async function compareRetained(engine,gpu,options,{samples=100,warmups=5,setupPhase=0}={}) {
  if(!Number.isInteger(samples)||samples<1||samples>1000||!Number.isInteger(warmups)||warmups<0||warmups>100)throw new Error('Invalid retained sample count');
  const reference=await engine.render(2,options), views={expanded:new RetainedSession(engine,gpu,10),shared:new RetainedSession(engine,gpu)};
  const first={},last={cpu:reference.data},rows=[],packets={},repeats={};
  try {
    const setupModes=['cpu','expanded','shared'],setupOrder=setupModes.slice(setupPhase%3).concat(setupModes.slice(0,setupPhase%3));
    first.order=setupOrder;
    for(const mode of setupOrder) {
      await engine.render(9,options);
      if(mode==='cpu') {const c=await engine.render(7,options);assertPixels(c.data,reference.data,'CPU retained first');first.cpu={...c.meta,frame_ms:c.host_ms};continue;}
      const start=performance.now(),entry=await views[mode].prepare(options);
      if(entry.fallback){first[mode]={fallback:entry.fallback,packet:entry.packet};continue;}
      const out=await entry.job.render(null);last[mode]=out.pixels;repeats[mode]=out.pixels;
      first[mode]={packet:entry.packet,resource_setup_ms:entry.setup_ms,upload_ms:entry.upload_ms,
        render_ms:entry.packet.engine_ms+entry.setup_ms+entry.upload_ms+out.wall_ms,frame_ms:performance.now()-start,
        packet_bytes:entry.packet_bytes,geometry:entry.geometry,counters:entry.job.counters()};
      // Repeat export proves packet identity independently of the uploaded copy.
      const packet=await engine.render(mode==='shared'?8:10,options);
      packets[mode]=await sha256(packet.data);
    }
    const modes=['cpu',...Object.keys(views).filter(m=>!first[m].fallback)];
    const trial=async mode=>{
      if(mode==='cpu'){
        const c=await engine.render(7,options);if(!c.meta.cache_hit)throw new Error('Warm CPU prepared scene was evicted');
        assertPixels(c.data,reference.data,'CPU prepared reuse');last.cpu=c.data;
        return {render_ms:c.meta.engine_ms,frame_ms:c.host_ms,prepare_ms:c.meta.prepare_ms,prepared_id:c.meta.prepared_id};
      }
      const requests=engine.renderRequests,start=performance.now(),out=await views[mode].render(options);
      const frame_ms=performance.now()-start;
      if(!out.cache_hit||engine.renderRequests!==requests)throw new Error('Warm GPU draw rebuilt/exported geometry');
      const repeat=equal(out.pixels,repeats[mode])?null:pixelDifference(out.pixels,repeats[mode]);
      last[mode]=out.pixels;
      return {render_ms:out.wall_ms,frame_ms,kernel_ms:out.kernel_ms,repeat};
    };
    for(let i=-warmups;i<samples;i++){
      const index=i+warmups,order=modes.slice(index%modes.length).concat(modes.slice(0,index%modes.length));if(Math.floor(index/modes.length)%2)order.reverse();
      const row={};for(const mode of order)row[mode]=await trial(mode);if(i>=0)rows.push(row);
    }
    const result={options,width:reference.meta.width,height:reference.meta.height,samples,warmups,first,packets,raw:rows,modes:{}};
    for(const mode of ['cpu','expanded','shared']){
      if(first[mode].fallback){result.modes[mode]={fallback:first[mode].fallback};continue;}
      const raw=rows.map(r=>r[mode]);result.modes[mode]={...summarize(raw),accuracy:pixelDifference(last[mode],reference.data),pixel_sha256:await sha256(last[mode])};
      if(mode!=='cpu'){
        const cache=views[mode].metadata(),counter=cache.resources[0];
        if(counter.uploads!==1||counter.dispatches!==samples+warmups+1)throw new Error('Retained upload/dispatch counts differ');
        result.modes[mode].cache=cache;result.modes[mode].repeat_variations=raw.filter(r=>r.repeat).length;
      }
    }
    result.cpu_cache=(await engine.render(7,options)).meta;
    return {result,pixels:last};
  }finally{for(const view of Object.values(views))await view.clear();}
}

export async function zoomRetained(engine,gpu,options,{samples=18,coldSamples=12,warmups=3,present=null,retained=null}={}) {
  const view=retained??new RetainedSession(engine,gpu),dpis=[options.dpi,Math.round(options.dpi*1.5),options.dpi*2],references=new Map(),accuracy=[];
  const cold={cpu:[],gpu:[]},warm={cpu:[],gpu:[]};let fallback=null;
  try {
    for(const dpi of dpis)references.set(dpi,await engine.render(2,{...options,dpi}));
    const trial=async(mode,dpi,i)=>{
      const target={...options,dpi},ref=references.get(dpi),start=performance.now();let pixels,render_ms,cache_hit,kernel_ms=null;
      if(mode==='cpu'){
        const out=await engine.render(7,target);pixels=out.data;render_ms=out.meta.engine_ms;cache_hit=out.meta.cache_hit;
      }else{
        const out=await view.render(target);
        if(out.fallback){fallback=out.fallback;return null;}
        pixels=out.pixels;cache_hit=out.cache_hit;kernel_ms=out.kernel_ms;
        render_ms=out.wall_ms+(cache_hit?0:out.entry.packet.engine_ms+out.entry.setup_ms+out.entry.upload_ms);
      }
      const ready_ms=performance.now()-start,position=i%2?'center':'top';
      if(present)await present(pixels,ref.meta.width,ref.meta.height,position);
      const frame_ms=performance.now()-start;
      if(mode==='cpu')assertPixels(pixels,ref.data,'CPU zoom prepared reuse');
      else accuracy.push({dpi,cached:cache_hit,...pixelDifference(pixels,ref.data)});
      return {dpi,position,render_ms,ready_ms,frame_ms,kernel_ms,cache_hit};
    };
    for(let i=0;i<coldSamples;i++)for(const mode of i%2?['gpu','cpu']:['cpu','gpu']){
      if(mode==='gpu'&&fallback)continue;
      await view.clear();await engine.render(9,options);
      const row=await trial(mode,dpis[i%dpis.length],i);if(row)cold[mode].push(row);
    }
    await view.clear();await engine.render(9,options);
    for(const dpi of dpis){await engine.render(7,{...options,dpi});if(!fallback){const e=await view.prepare({...options,dpi});if(e.fallback)fallback=e.fallback;}}
    const before=view.metadata();
    for(let i=-warmups;i<samples;i++)for(const mode of i%2?['gpu','cpu']:['cpu','gpu']){
      if(mode==='gpu'&&fallback)continue;
      const row=await trial(mode,dpis[(i+warmups)%dpis.length],i);
      if(!row.cache_hit)throw new Error('Warm zoom resolution was not retained');
      if(i>=0)warm[mode].push(row);
    }
    const after=view.metadata();
    if(after.resources.some(r=>r.uploads!==1))throw new Error('Warm zoom reuploaded geometry');
    const summarizeModes=all=>Object.fromEntries(Object.entries(all).filter(([,rows])=>rows.length).map(([mode,rows])=>[mode,{...summarize(rows),samples:rows}]));
    return {dpis,cold:summarizeModes(cold),warm:summarizeModes(warm),accuracy,fallback,gpu_cache_before:before,gpu_cache_after:after,
      cpu_cache:(await engine.render(7,{...options,dpi:dpis.at(-1)})).meta,
      timing:present?'Completed readback and canvas upload to two animation-frame opportunities; not display scanout':'Completed RGBA readback/copy; native IPC in frame_ms',
      cold_policy:'Prepared CPU and GPU resource caches cleared before each mode/target; recorded scene, Store and pipelines stay warm'};
  }finally{if(!retained)await view.clear();}
}
