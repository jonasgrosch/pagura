/** World geometry, sparse patch and CPU parity against the hardware browser. */
import {readFile,readdir,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {Host} from './transport.mjs';
import {MetalBackend} from './gpu.mjs';
import {WorldSession} from '../world.mjs';
import {sha256} from '../retained-study.mjs';
import {pixelDifference} from '../svg.mjs';
const option=(key,fallback)=>process.argv.includes(key)?process.argv[process.argv.indexOf(key)+1]:fallback;
const root=resolve(import.meta.dirname,'../../..');
const engine=new Host(resolve(option('--binary','target/gpu-demo/release/pagura-gpu-demo')));
const host=new Host(resolve(option('--metal','target/gpu-demo/metal-host')),[resolve(root,'benchmarks/gpu-demo/native/raster.metal')],true);
const cases=[],failures=[];
try {
  const info=(await host.ready).meta,gpu=new MetalBackend(host,info);
  const browser=JSON.parse(await readFile(resolve(option('--browser-report','target/gpu-demo/display-fixtures.json'))));
  const fixtures=new Map();
  for(const file of await readdir(resolve(root,'corpus/manifests')))if(file.endsWith('.json'))for(const entry of JSON.parse(await readFile(resolve(root,'corpus/manifests',file))).cases)fixtures.set(entry.id,entry);
  for(const entry of browser.cases){
    const fixture=fixtures.get(entry.id),options={page:0,dpi:fixture.dpi??72},world=new WorldSession(engine,gpu);
    try {
      await engine.open(await readFile(resolve(root,fixture.path)));
      const cpu=await engine.render(7,options);if(await sha256(cpu.data)!==entry.cpu_sha256)throw new Error('Native/WASM CPU mismatch');
      const output=await world.draw(options,{readback:true});
      if(Boolean(output.fallback)!==Boolean(entry.fallback))throw new Error('Native/WASM world admission mismatch');
      if(output.fallback){cases.push({id:entry.id,fallback:output.fallback});continue;}
      const packet=await engine.render(11,options),patches=await engine.render(12,options);
      if(await sha256(packet.data)!==entry.world_sha256||await sha256(patches.data)!==entry.patch_sha256)throw new Error('Native/WASM world or view patch mismatch');
      const accuracy=pixelDifference(output.pixels,cpu.data);if(accuracy.max_channel_error>2)throw new Error('Native world accuracy exceeded two levels');
      const zoom_accuracy=[];
      for(const target of entry.zoom_accuracy??[]){
        const opt={...options,dpi:target.dpi},reference=await engine.render(7,opt),out=await world.draw(opt,{readback:true});
        if(out.fallback)throw new Error('Native world zoom admission changed');const delta=pixelDifference(out.pixels,reference.data);
        if(delta.max_channel_error>2)throw new Error('Native world zoom accuracy exceeded two levels');zoom_accuracy.push({dpi:target.dpi,...delta});
      }
      if(world.job.metadata().geometry_uploads!==1)throw new Error('Native world reuploaded geometry');
      cases.push({id:entry.id,accuracy,zoom_accuracy});
    }catch(error){failures.push({id:entry.id,error:String(error)});}
    finally{await world.clear();}
    if(cases.length%150===0)console.log('Native world fixtures',cases.length,browser.cases.length);
  }
  const report={timestamp:new Date().toISOString(),capability:info,fixtures:browser.fixtures,admitted:cases.filter(c=>c.accuracy).length,cases,failures};
  await writeFile(resolve(option('--output','target/gpu-demo/display-native-fixtures.json')),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify({cases:cases.length,admitted:report.admitted,failures}));if(failures.length)process.exitCode=1;
}finally{engine.close();host.close();}
