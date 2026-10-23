import {demo} from './app.mjs';
import {DisplayStudy} from './display-study.mjs';
import {browserHarness} from './display-browser.mjs';
import {sha256} from './retained-study.mjs';
export async function run(config){
  await demo.ready;const {engine,gpu}=demo;if(!gpu)throw new Error('Hardware GPU required');
  if(config.mode==='world-ui-test')return(await import('./display-tests.mjs')).runUi(demo);
  if(config.mode==='world-test')return(await import('./display-tests.mjs')).runTests(engine,gpu,config);
  const harness=await browserHarness(gpu),cases=[],failures=[];
  try {
    for(const input of await(await fetch('/benchmark-inputs.json')).json()){
      const bytes=new Uint8Array(await(await fetch(input.path)).arrayBuffer()),input_sha256=await sha256(bytes);await engine.open(bytes);
      const study=new DisplayStudy(engine,gpu,harness);
      try {
        for(const dpi of config.dpis){
          console.log('Display',input.label,dpi);const out=await study.compare({page:0,dpi},config);Object.assign(out,{label:input.label,input_sha256});cases.push(out);
          if(dpi===150&&config.zoom){console.log('Novel and revisited zoom',input.label);out.zoom=await study.zoom({page:0,dpi});}
        }
      }finally{await study.clear();}
    }
  }finally{harness.destroy();}
  return {adapter:gpu.info,cases,failures,summary:cases.map(c=>({label:c.label,dpi:c.options.dpi,modes:c.modes}))};
}
