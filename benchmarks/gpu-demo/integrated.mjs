/** Equivalent document opening, retained-pixel scrolling and novel zoom study. */
import {Engine} from './engine.mjs';
import {RetainedSession} from './retained.mjs';
import {WorldSession} from './world.mjs';
import {Presenter,viewport} from './present.mjs';
import {paintOpportunity} from './zoom.mjs';
import {stats,assertPixels} from './measure.mjs';
import {sha256} from './retained-study.mjs';
import {pixelDifference} from './svg.mjs';
const modes=['cpu','retained','world'],dpis=[72,150,300],novel=[151,163,177,191,207,221,239,253,269,281,293,299];
const orderAt=(values,index)=>{const shift=index%values.length,order=values.slice(shift).concat(values.slice(0,shift));return Math.floor(index/values.length)%2?order.reverse():order;};
export class BrowserPage {
  static async create(mode,gpu,bytes,container=null,options={}) {
    const started=performance.now(),engine=await Engine.create(),launch_ms=performance.now()-started;
    try {
      const page=new BrowserPage(mode,gpu,engine,container,options);if(mode!=='cpu')page.presenter=await Presenter.create(gpu,page.gpuCanvas);
      const open=await engine.open(bytes);page.opening={launch_ms,open_request_ms:open.host_ms,open_engine_ms:open.meta.open_ms};return page;
    }catch(error){engine.destroy();throw error;}
  }
  constructor(mode,gpu,engine,container,options) {
    Object.assign(this,{mode,gpu,engine,retained:new RetainedSession(engine,gpu),world:new WorldSession(engine,gpu,options),fallback:null});
    this.ownsStage=!container;this.stage=container??document.createElement('div');
    if(this.ownsStage){this.stage.className='zoom-stage';this.stage.dataset.integratedStudy='';document.body.append(this.stage);}
    this.cpuCanvas=document.createElement('canvas');this.gpuCanvas=document.createElement('canvas');this.stage.append(this.cpuCanvas,this.gpuCanvas);
    this.cpuCanvas.width=480;this.cpuCanvas.height=480;this.context=this.cpuCanvas.getContext('2d');
    this.bitmap=document.createElement('canvas');this.bitmapContext=this.bitmap.getContext('2d');
  }
  activate(){if(this.ownsStage){for(const stage of document.querySelectorAll('[data-integrated-study]'))stage.style.display=stage===this.stage?'block':'none';}const cpu=this.mode==='cpu'||!!this.fallback;this.cpuCanvas.style.display=cpu?'block':'none';this.gpuCanvas.style.display=cpu?'none':'block';}
  async render(dpi,{present=true}={}) {
    const started=performance.now();let out,cache_hit=false,render_ms=0;
    if(this.presenter)this.presenter.position='top';
    if(this.mode==='retained'&&!this.fallback){
      const entry=await this.retained.prepare({dpi});
      if(entry.fallback)this.fallback=entry.fallback;
      else {this.job=entry.job;cache_hit=entry.cache_hit;out=await entry.job.render(null,{readback:false,presenter:present?this.presenter:null});this.width=entry.packet.width;this.height=entry.packet.height;this.buffer=entry.job.output_buffer;render_ms=performance.now()-started;}
    }else if(this.mode==='world'&&!this.fallback){
      out=await this.world.draw({dpi},{presenter:present?this.presenter:null});
      if(out.fallback)this.fallback=out.fallback;
      else {cache_hit=out.view_cache_hit;this.width=out.width;this.height=out.height;this.buffer=this.world.job.output_buffer;render_ms=performance.now()-started;}
    }
    if(this.mode==='cpu'||this.fallback){
      out=await this.engine.render(7,{dpi});cache_hit=out.meta.cache_hit;this.width=out.meta.width;this.height=out.meta.height;
      if(this.bitmap.width!==this.width||this.bitmap.height!==this.height){this.bitmap.width=this.width;this.bitmap.height=this.height;}
      this.bitmapContext.putImageData(new ImageData(new Uint8ClampedArray(out.data),this.width,this.height),0,0);
      render_ms=performance.now()-started;if(present)this.paintCpu({x:0,y:0});
    }
    this.activate();this.dpi=dpi;if(present)await paintOpportunity();
    return {frame_ms:performance.now()-started,render_ms,cache_hit,fallback:this.fallback};
  }
  paintCpu(offset){this.context.fillStyle='white';this.context.fillRect(0,0,480,480);this.context.drawImage(this.bitmap,-offset.x,-offset.y);}
  async scroll(fraction,{capture=false}={}) {
    const offset={x:Math.max(0,Math.floor((this.width-480)/2)),y:Math.floor(Math.max(0,this.height-480)*fraction)},started=performance.now();this.activate();
    if(this.mode==='cpu'||this.fallback)this.paintCpu(offset);
    else {
      this.presenter.position=offset;this.presenter.captureNext=capture;
      const encoder=this.gpu.device.createCommandEncoder();this.presenter.encode(encoder,this.buffer,this.width,this.height);this.gpu.device.queue.submit([encoder.finish()]);await this.gpu.device.queue.onSubmittedWorkDone();
    }
    await paintOpportunity();
    return {frame_ms:performance.now()-started,offset,pixels:capture?(this.mode==='cpu'||this.fallback?new Uint8Array(this.context.getImageData(0,0,480,480).data):await this.presenter.capture()):null};
  }
  counters(){return {engine_requests:this.engine.renderRequests??0,retained:this.retained.metadata(),world:this.world.job?.metadata()??null};}
  async accuracy(dpi) {
    const reference=await this.engine.render(2,{dpi}),cached=await this.engine.render(7,{dpi});assertPixels(cached.data,reference.data,'Integrated CPU prepared output');
    await this.render(dpi,{present:false});let pixels;
    if(this.mode==='cpu'||this.fallback)pixels=cached.data;
    else if(this.mode==='retained')pixels=(await this.job.render(null)).pixels;
    else pixels=(await this.world.draw({dpi},{readback:true})).pixels;
    const captures=[];
    for(const fraction of [0,0.5,1]){const capture=await this.scroll(fraction,{capture:true}),delta=pixelDifference(capture.pixels,viewport(pixels,this.width,this.height,480,capture.offset));if(delta.max_channel_error)throw new Error('Integrated presentation changed pixels');captures.push({fraction,difference:delta});}
    return {dpi,width:this.width,height:this.height,cpu_sha256:await sha256(reference.data),pixels_sha256:await sha256(pixels),difference:pixelDifference(pixels,reference.data),captures};
  }
  async destroy(){await this.retained.clear();await this.world.clear();this.presenter?.destroy();this.engine.destroy();if(this.ownsStage)this.stage.remove();else this.stage.replaceChildren();}
}
export async function run(config){
  const {demo}=await import('./app.mjs');await demo.ready;const gpu=demo.gpu;if(!gpu)throw new Error('Hardware WebGPU required');
  if(config.mode==='integrated-ui-test')return runUi(demo);
  const {samples=100,warmups=5,coldSamples=20}=config;
  if(!Number.isInteger(samples)||samples<1||samples>1000||!Number.isInteger(warmups)||warmups<0||warmups>100||!Number.isInteger(coldSamples)||coldSamples<1||coldSamples>100)throw new Error('Integrated sample bounds');
  const cases=[];
  for(const input of await(await fetch('/benchmark-inputs.json')).json()) {
    const read=performance.now(),bytes=new Uint8Array(await(await fetch(input.path)).arrayBuffer()),fileMs=performance.now()-read,input_sha256=await sha256(bytes);
    const opening=Object.fromEntries(modes.map(mode=>[mode,[]])),pages=[];
    console.log('Integrated opening',input.label);
    for(let i=0;i<coldSamples;i++)for(const mode of orderAt(modes,i)){
      const started=performance.now(),page=await BrowserPage.create(mode,gpu,bytes,null,config);
      try {const draw=await page.render(150);opening[mode].push({...page.opening,first_display_ms:performance.now()-started,draw});}finally{await page.destroy();}
    }
    try {
      for(const mode of modes)pages.push(await BrowserPage.create(mode,gpu,bytes,null,config));
      const statics=[];
      for(const dpi of dpis){
        console.log('Integrated redraw',input.label,dpi);const rows=Object.fromEntries(modes.map(mode=>[mode,[]]));
        for(let i=-warmups;i<samples;i++)for(const mode of orderAt(modes,i+warmups)){const page=pages[modes.indexOf(mode)],row=await page.render(dpi);if(i>=0)rows[mode].push(row);}
        statics.push({dpi,raw:rows,summary:Object.fromEntries(modes.map(mode=>[mode,{frame_ms:stats(rows[mode].map(r=>r.frame_ms)),render_ms:stats(rows[mode].map(r=>r.render_ms))}]))});
      }
      const zoom=Object.fromEntries(modes.map(mode=>[mode,[]]));
      for(const dpi of novel)for(const mode of orderAt(modes,dpi)){
        const page=pages[modes.indexOf(mode)];await page.engine.render(9,{dpi});zoom[mode].push({dpi,...await page.render(dpi)});
      }
      for(const page of pages)await page.render(300);
      const before=pages.map(p=>p.counters()),scrolls=Object.fromEntries(modes.map(m=>[m,[]]));
      for(let i=-warmups;i<samples;i++)for(const mode of orderAt(modes,i+warmups)){
        const page=pages[modes.indexOf(mode)],row=await page.scroll(((i+warmups)%60)/59);delete row.pixels;if(i>=0)scrolls[mode].push(row);
      }
      const after=pages.map(p=>p.counters());if(JSON.stringify(before)!==JSON.stringify(after))throw new Error('Scrolling rendered or uploaded page content');
      for(const [n,page] of pages.entries()){
        console.log('Integrated accuracy',input.label,page.mode);const scroll=scrolls[page.mode];
        const accuracy=[];for(const dpi of [...dpis,...novel])accuracy.push(await page.accuracy(dpi));
        cases.push({label:input.label,mode:page.mode,input_sha256,input_file_read_ms:fileMs,fallback:page.fallback,opening_raw:opening[page.mode],opening:stats(opening[page.mode].map(r=>r.first_display_ms)),static:statics.map(s=>({dpi:s.dpi,raw:s.raw[page.mode],summary:s.summary[page.mode]})),novel_zoom_raw:zoom[page.mode],novel_zoom:stats(zoom[page.mode].map(r=>r.frame_ms)),scroll_raw:scroll,scroll:stats(scroll.map(r=>r.frame_ms)),scroll_before:before[n],scroll_after:after[n],accuracy});
      }
    }finally{for(const page of pages)await page.destroy();}
  }
  return {reciprocal:!!config.reciprocal,adapter:gpu.info,samples,warmups,cold_samples:coldSamples,clock:'Document-cold, application/adapter warm; worker start/WASM compile/open through two animation-frame opportunities. File fetch separate. Not scanout.',cases,failures:[],summary:cases.map(c=>({label:c.label,mode:c.mode,fallback:c.fallback,opening:c.opening,zoom:c.novel_zoom,scroll:c.scroll}))};
}
async function runUi(demo){
  const wait=async predicate=>{const end=performance.now()+120000;while(!predicate()){if(performance.now()>end)throw new Error('Integrated UI timeout');await new Promise(r=>setTimeout(r,25));}};
  document.querySelector('#controls').requestSubmit();await wait(()=>!demo.busy&&demo.current);
  document.querySelector('#integrated-run').click();await wait(()=>!demo.busy&&demo.lastIntegrated);
  if(demo.lastIntegrated.length!==3||document.querySelector('#integrated-results').children.length!==3)throw new Error('Integrated comparison missing modes');
  const slider=document.querySelector('#integrated-scroll');if(slider.disabled)throw new Error('Integrated scroll unavailable');slider.value='88';slider.dispatchEvent(new Event('input'));await wait(()=>!demo.busy);
  if(demo.lastIntegrated[0].accuracy.difference.max_channel_error!==0)throw new Error('Integrated CPU output changed');
  return {failures:[],modes:demo.lastIntegrated.map(r=>({name:r.name,fallback:r.fallback,max_channel_error:r.accuracy.difference.max_channel_error})),scroll:88};
}
