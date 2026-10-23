/** Interactive viewer experiment; formal runners use the same portable study. */
import {DisplayStudy} from './display-study.mjs';
import {browserHarness} from './display-browser.mjs';
const label={cpu:'CPU prepared scene → Canvas',retained_readback:'Retained GPU → readback → Canvas',retained_direct:'Retained GPU → direct display',world_direct:'World geometry + view patches → direct display'};
const ms=v=>`${v.toFixed(2)} ms`;
export function installDirectPanel(demo) {
  const $=id=>document.getElementById(id);let study=null,harness=null,pendingDpi=null;
  const buttons=value=>{for(const id of ['display-run','display-zoom'])$(id).disabled=value;};
  const message=value=>{$('display-status').textContent=value;};
  const options=()=>({...demo.current.output.result.options,dpi:Number($('display-dpi').value)});
  async function ensure(){
    if(!study){
      harness=await browserHarness(demo.gpu,{cpuCanvas:$('display-cpu'),gpuCanvas:$('display-gpu')});
      study=new DisplayStudy(demo.engine,demo.gpu,harness,{retained:demo.current.output.retained??null});
    }return study;
  }
  async function preview(target){
    const cpu=await demo.engine.render(7,target);await harness.cpu(cpu.data,cpu.meta.width,cpu.meta.height,'top');
    const world=await study.world.prepare(target);let mode=world.fallback?'retained_direct':'world_direct';
    let out=await study.trial(mode,target);
    if(out.fallback&&mode==='world_direct'){mode='retained_direct';out=await study.trial(mode,target);}
    $('display-gpu-caption').textContent=out.fallback?'GPU unavailable · CPU fallback':mode==='world_direct'?'World geometry · direct GPU viewport':'Resolution-specific geometry · direct GPU viewport';
    $('display-target').textContent=`${target.dpi} DPI`;return out;
  }
  async function compare(){
    if(demo.busy||!demo.current||!demo.gpu)return;
    demo.busy=true;buttons(true);message('Measuring fresh pixels and direct display…');
    try {
      await ensure();const result=await study.compare(options(),{samples:9,warmups:2});demo.lastDisplay=result;
      $('display-results').replaceChildren();
      for(const mode of Object.keys(label)){
        const row=document.createElement('tr'),timing=result.modes[mode],accuracy=mode==='cpu'?{max_channel_error:0}:result.accuracy[mode.startsWith('world')?'world':'retained'];
        const cells=[label[mode],timing?ms(timing.render_ms.median):'CPU fallback',timing?`${ms(timing.frame_ms.median)} / ${ms(timing.frame_ms.p95)}`:'—',accuracy?.fallback??(accuracy?`${accuracy.max_channel_error} levels`:'—')];
        for(const text of cells){const td=document.createElement('td');td.textContent=text;row.append(td);}$('display-results').append(row);
      }
      const counter=result.after.world,prior=result.before.world;
      message(counter?`9 samples after 2 warmups. World geometry uploads: ${counter.geometry_uploads}; timed RGBA readbacks: ${counter.rgba_readbacks-prior.rgba_readbacks}. New DPIs still prepare sparse CPU view corrections. Accuracy readbacks run separately.`:'GPU admission is unavailable for this page; the complete CPU result is retained.');
      $('display-dpi').disabled=false;await preview(options());
    }catch(error){message(String(error));}
    finally{demo.busy=false;buttons(false);if(pendingDpi!==null)$('display-dpi').dispatchEvent(new Event('input'));}
  }
  $('display-run').addEventListener('click',compare);
  $('display-zoom').addEventListener('click',async()=>{
    if(demo.busy||!demo.current||!demo.gpu)return;
    demo.busy=true;buttons(true);message('Measuring 12 novel targets, then three retained targets…');
    try {
      await ensure();const result=await study.zoom({...options(),dpi:150});demo.lastDisplayZoom=result;
      const entries=Object.keys(result.novel).map(mode=>`${label[mode]}: new ${ms(result.novel[mode].frame_ms.median)}, revisited ${ms(result.warm[mode].frame_ms.median)}`);
      message(entries.join(' · ')+'. Browser frame opportunities, not scanout. CPU and GPU each retain at most three views.');await preview(options());
    }catch(error){message(String(error));}
    finally{demo.busy=false;buttons(false);if(pendingDpi!==null)$('display-dpi').dispatchEvent(new Event('input'));}
  });
  $('display-dpi').addEventListener('input',async()=>{
    pendingDpi=Number($('display-dpi').value);$('display-target').textContent=`${pendingDpi} DPI`;
    if(demo.busy||!study)return;
    demo.busy=true;buttons(true);
    try {
      while(pendingDpi!==null){const dpi=pendingDpi;pendingDpi=null;await preview({...options(),dpi});}
      message('Live preview shares CPU preparation between panels. Use the comparison and zoom buttons for isolated measurements. Timed GPU display does not read back RGBA.');
    }catch(error){message(String(error));}
    finally{demo.busy=false;buttons(false);if(pendingDpi!==null)$('display-dpi').dispatchEvent(new Event('input'));}
  });
  return {
    enable:()=>{buttons(!demo.gpu);$('display-dpi').value=demo.current?.output.result.options.dpi??150;},
    clear:async()=>{buttons(true);$('display-dpi').disabled=true;pendingDpi=null;await study?.clear();harness?.destroy();study=null;harness=null;},
  };
}
