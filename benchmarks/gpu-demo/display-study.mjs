/** Same CPU/GPU trials on both hosts; presentation clocks are host-specific. */
import {RetainedSession} from './retained.mjs';
import {WorldSession} from './world.mjs';
import {stats,assertPixels} from './measure.mjs';
import {sha256} from './retained-study.mjs';
import {pixelDifference} from './svg.mjs';
import {viewport} from './present.mjs';
const summarize=rows=>{
  const result=Object.fromEntries(['render_ms','frame_ms','kernel_ms'].filter(k=>rows[0]?.[k]!=null).map(k=>[k,stats(rows.map(r=>r[k]))]));
  if(rows.some(r=>r.presentation?.drawable_presented!=null)){
    const presented=rows.filter(r=>r.presentation?.drawable_presented),dropped=rows.length-presented.length;
    result.drawable={presented:presented.length,dropped,presented_ms:presented.length?stats(presented.map(r=>r.presentation.drawable_presented_ms)):null};
  }return result;
};
const orderAt=(modes,index)=>{const order=modes.slice(index%modes.length).concat(modes.slice(0,index%modes.length));return Math.floor(index/modes.length)%2?order.reverse():order;};
export class DisplayStudy {
  constructor(engine,gpu,harness,{retained=null}={}){Object.assign(this,{engine,gpu,harness,ownsRetained:!retained,retained:retained??new RetainedSession(engine,gpu),world:new WorldSession(engine,gpu)});}
  async trial(mode,options,position='top') {
    this.harness.activate(mode);this.harness.presenter.position=position;
    const start=performance.now();let render_ms,kernel_ms=null,out,cache_hit,patch_meta=null,presentation=null;
    if(mode==='cpu'){
      const cpu=await this.engine.render(7,options);render_ms=cpu.meta.engine_ms;cache_hit=cpu.meta.cache_hit;
      presentation=await this.harness.cpu(cpu.data,cpu.meta.width,cpu.meta.height,position);out=cpu;
    }else if(mode==='world_direct'){
      out=await this.world.draw(options,{presenter:this.harness.presenter});if(out.fallback)return out;
      render_ms=(out.device_ms??out.gpu_ms??out.wall_ms)+(out.patch_meta?.engine_ms??0);kernel_ms=out.kernel_ms;cache_hit=out.view_cache_hit;patch_meta=out.patch_meta;
      await this.harness.complete();
    }else{
      const entry=await this.retained.prepare(options);if(entry.fallback)return entry;
      const readback=mode==='retained_readback',before=entry.job.counters();
      out=await entry.job.render(null,{readback,presenter:readback?null:this.harness.presenter});
      render_ms=out.wall_ms+(entry.cache_hit?0:entry.packet.engine_ms+entry.setup_ms+entry.upload_ms);
      kernel_ms=out.kernel_ms;cache_hit=entry.cache_hit;
      if(!readback&&entry.job.counters().rgba_readbacks!==before.rgba_readbacks)throw new Error('Direct retained draw copied RGBA to CPU');
      if(readback)presentation=await this.harness.cpu(out.pixels,entry.packet.width,entry.packet.height,position);else await this.harness.complete();
    }
    return {render_ms,frame_ms:performance.now()-start,kernel_ms,cache_hit,patch_meta,presentation:presentation??out?.presentation??null};
  }
  async accuracy(options) {
    const reference=await this.engine.render(2,options),cached=await this.engine.render(7,options);assertPixels(cached.data,reference.data,'CPU prepared display');
    const result={cpu_sha256:await sha256(reference.data),width:reference.meta.width,height:reference.meta.height};
    result.cpu_presentation=[];
    for(const position of ['top','center']){
      this.harness.activate('cpu');const capture=await this.harness.captureCpu(reference.data,reference.meta.width,reference.meta.height,position);
      const delta=pixelDifference(capture,viewport(reference.data,reference.meta.width,reference.meta.height,480,position));
      if(delta.max_channel_error)throw new Error('CPU presentation changed pixels');result.cpu_presentation.push({position,...delta});
    }
    const retained=await this.retained.prepare(options),world=await this.world.prepare(options);
    for(const [mode,entry] of [['retained',retained],['world',world]]){
      if(entry.fallback){result[mode]={fallback:entry.fallback};continue;}
      const output=mode==='world'?await this.world.draw(options,{readback:true}):await entry.job.render(null);
      if(output.fallback){result[mode]={fallback:output.fallback};continue;}
      result[mode]={...pixelDifference(output.pixels,reference.data),pixel_sha256:await sha256(output.pixels),presentation:[]};
      if(mode==='world'){result.world.geometry_sha256=await sha256((await this.engine.render(11,options)).data);result.world.patch_sha256=await sha256((await this.engine.render(12,options)).data);}
      for(const position of ['top','center']){
        this.harness.activate(mode);this.harness.presenter.position=position;this.harness.presenter.captureNext=true;
        if(mode==='world')await this.world.draw(options,{presenter:this.harness.presenter});else await entry.job.render(null,{readback:false,presenter:this.harness.presenter});
        const capture=await this.harness.presenter.capture(),expected=viewport(output.pixels,reference.meta.width,reference.meta.height,480,position);
        const delta=pixelDifference(capture,expected);if(delta.max_channel_error)throw new Error('Presentation changed GPU pixels');result[mode].presentation.push({position,...delta});
      }
    }
    return result;
  }
  async compare(options,{samples=100,warmups=5}={}) {
    if(!Number.isInteger(samples)||samples<1||samples>1000||!Number.isInteger(warmups)||warmups<0||warmups>100)throw new Error('Invalid display samples');
    const first={};await this.engine.render(9,options);
    let start=performance.now();const cpu=await this.engine.render(7,options);first.cpu={...cpu.meta,request_ms:performance.now()-start};
    await this.engine.render(9,options);start=performance.now();const retained=await this.retained.prepare(options);
    first.retained={...retained.packet,setup_request_ms:performance.now()-start,cache_hit:retained.cache_hit,fallback:retained.fallback};
    await this.engine.render(9,options);start=performance.now();const world=await this.world.prepare(options);
    first.world={...this.world.packet,setup_request_ms:performance.now()-start,cache_hit:world.cache_hit,fallback:world.fallback};
    const modes=['cpu',...(retained.fallback?[]:['retained_readback','retained_direct']),...(world.fallback?[]:['world_direct'])];
    for(const mode of modes){if(mode==='world_direct')await this.engine.render(9,options);first[mode+'_draw']=await this.trial(mode,options);}
    const before={retained:this.retained.metadata(),world:this.world.job?.metadata()},rows=[];
    for(let i=-warmups;i<samples;i++){
      const row={};for(const mode of orderAt(modes,i+warmups)){
        const requests=this.engine.renderRequests;row[mode]=await this.trial(mode,options);
        if(row[mode].fallback||!row[mode].cache_hit)throw new Error('Static display did not reuse prepared resources');
        if(mode!=='cpu'&&this.engine.renderRequests!==requests)throw new Error('Warm GPU display requested CPU geometry');
      }if(i>=0)rows.push(row);
    }
    const after={retained:this.retained.metadata(),world:this.world.job?.metadata()};
    if(after.world&&after.world.rgba_readbacks!==before.world.rgba_readbacks)throw new Error('Direct world draw copied RGBA to CPU');
    const timing=Object.fromEntries(modes.map(mode=>[mode,summarize(rows.map(r=>r[mode]))]));
    const accuracy=await this.accuracy(options);
    return {options,samples,warmups,first,modes:timing,raw:rows,before,after,accuracy,cpu_cache:(await this.engine.render(7,options)).meta,clock:this.harness.clock};
  }
  async zoom(options,{samples=18,warmups=3,novelDpis=[151,163,177,191,207,221,239,253,269,281,293,299]}={}) {
    const dpis=[150,225,300],available=['cpu'];
    const retained=await this.retained.prepare(options),world=await this.world.prepare(options);
    if(!retained.fallback)available.push('retained_direct');if(!world.fallback)available.push('world_direct');
    const novel={},warm={},accuracy=[],before={retained:this.retained.metadata(),world:this.world.job?.metadata()};
    for(const mode of available){novel[mode]=[];warm[mode]=[];}
    for(let i=0;i<novelDpis.length;i++)for(const mode of orderAt(available,i)){
      await this.engine.render(9,options);
      const row=await this.trial(mode,{...options,dpi:novelDpis[i]},i%2?'center':'top');
      if(row.fallback)throw new Error(`Novel view refused: ${row.fallback}`);
      if(row.cache_hit)throw new Error('Novel target reused a DPI entry');novel[mode].push({...row,dpi:novelDpis[i]});
    }
    const afterNovel={retained:this.retained.metadata(),world:this.world.job?.metadata()};
    await this.engine.render(9,options);
    for(const dpi of dpis)for(const mode of available)await this.trial(mode,{...options,dpi});
    const beforeWarm={retained:this.retained.metadata(),world:this.world.job?.metadata()};
    for(let i=-warmups;i<samples;i++)for(const mode of orderAt(available,i+warmups)){
      const dpi=dpis[(i+warmups)%3],row=await this.trial(mode,{...options,dpi},i%2?'center':'top');
      if(!row.cache_hit)throw new Error('Warm display target was not retained');if(i>=0)warm[mode].push({...row,dpi});
    }
    const after={retained:this.retained.metadata(),world:this.world.job?.metadata()};
    if(after.world&&(after.world.geometry_uploads!==1||after.world.rgba_readbacks!==beforeWarm.world.rgba_readbacks))throw new Error('World direct zoom transferred geometry or RGBA');
    // Differential readbacks are deliberately after all timed display updates.
    for(const dpi of [...novelDpis,...dpis])accuracy.push({dpi,...await this.accuracy({...options,dpi})});
    const summary=all=>Object.fromEntries(Object.entries(all).map(([mode,rows])=>[mode,{...summarize(rows),samples:rows}]));
    return {novel_dpis:novelDpis,retained_dpis:dpis,novel:summary(novel),warm:summary(warm),before,after_novel:afterNovel,before_warm:beforeWarm,after,accuracy,clock:this.harness.clock};
  }
  async clear(){if(this.ownsRetained)await this.retained.clear();await this.world.clear();}
}
