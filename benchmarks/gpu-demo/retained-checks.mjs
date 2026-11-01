import {demo} from './app.mjs';
import {compareRetained,sha256} from './retained-study.mjs';
import {browserZoom} from './retained-zoom.mjs';
export async function run(config){
  await demo.ready;const {engine,gpu}=demo;if(!gpu)throw new Error('Hardware GPU required');
  if(config.mode==='retained-test'){const {runTests}=await import('./retained-tests.mjs');return runTests(engine,gpu);}
  const cases=[],artifacts=[];
  for(const input of await(await fetch('/benchmark-inputs.json')).json()){
    const pdf=new Uint8Array(await(await fetch(input.path)).arrayBuffer()),input_sha256=await sha256(pdf);await engine.open(pdf);
    for(const dpi of config.dpis){
      console.log(`Retained ${input.label} ${dpi} DPI`);
      const output=await compareRetained(engine,gpu,{page:0,dpi},{...config,setupPhase:cases.length});
      Object.assign(output.result,{label:input.label,input_sha256});cases.push(output.result);
      if(dpi===150&&config.zoom){console.log(`Zoom ${input.label}`);output.result.zoom=await browserZoom(engine,gpu,{page:0,dpi},{samples:18,coldSamples:12,warmups:3});}
    }
  }
  return {adapter:gpu.info,cases,artifacts,failures:[],summary:cases.map(c=>({label:c.label,dpi:c.options.dpi,modes:c.modes}))};
}
