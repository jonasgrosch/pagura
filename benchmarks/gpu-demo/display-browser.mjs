import {Presenter,viewport} from './present.mjs';
import {paintOpportunity} from './zoom.mjs';
export async function browserHarness(gpu,{cpuCanvas=null,gpuCanvas=null}={}) {
  let stage=null;const attached=!!cpuCanvas;
  if(!attached){stage=document.createElement('div');stage.className='zoom-stage';document.body.append(stage);cpuCanvas=document.createElement('canvas');gpuCanvas=document.createElement('canvas');stage.append(cpuCanvas,gpuCanvas);}
  cpuCanvas.width=480;cpuCanvas.height=480;const context=cpuCanvas.getContext('2d'),presenter=await Presenter.create(gpu,gpuCanvas);
  return {presenter,clock:'Request through completed pixels and two browser animation-frame opportunities; not scanout',
    activate:mode=>{if(!attached){const cpu=mode==='cpu'||mode==='retained_readback';cpuCanvas.style.display=cpu?'block':'none';gpuCanvas.style.display=cpu?'none':'block';}},
    cpu:async(pixels,width,height,position)=>{context.putImageData(new ImageData(new Uint8ClampedArray(viewport(pixels,width,height,480,position)),480,480),0,0);await paintOpportunity();},
    captureCpu:async(pixels,width,height,position)=>{
      context.putImageData(new ImageData(new Uint8ClampedArray(viewport(pixels,width,height,480,position)),480,480),0,0);await paintOpportunity();
      return new Uint8Array(context.getImageData(0,0,480,480).data);
    },
    complete:paintOpportunity,
    destroy:()=>{presenter.destroy();stage?.remove();},
  };
}
