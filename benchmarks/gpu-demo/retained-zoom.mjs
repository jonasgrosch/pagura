/** Actual fresh-resolution Canvas presentation in a clipped viewport. */
import {paintOpportunity,putPixels} from './zoom.mjs';
import {zoomRetained} from './retained-study.mjs';
export async function browserZoom(engine,gpu,options,config={}) {
  const stage=document.createElement('div');stage.className='zoom-stage';document.body.append(stage);
  const canvas=document.createElement('canvas');stage.append(canvas);
  try {
    return await zoomRetained(engine,gpu,options,{...config,present:async(pixels,width,height,position)=>{
      putPixels(canvas,pixels,width,height);
      stage.scrollLeft=position==='center'?Math.max(0,(width-480)/2):0;
      stage.scrollTop=position==='center'?Math.max(0,(height-480)/2):0;
      await paintOpportunity();
    }});
  }finally{stage.remove();}
}
