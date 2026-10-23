/** Native counterpart of the browser's retained CPU/GPU study. */
import {readFile,readdir,writeFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {resolve,dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {Host} from './transport.mjs';
import {MetalBackend} from './gpu.mjs';
import {compareRetained,zoomRetained,sha256} from '../retained-study.mjs';
import {pixelDifference} from '../svg.mjs';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'../../..');
const option=(k,d)=>process.argv.includes(k)?process.argv[process.argv.indexOf(k)+1]:d;
const binary=resolve(option('--binary','target/gpu-demo/release/pagura-gpu-demo')),metalBinary=resolve(option('--metal','target/gpu-demo/metal-host'));
const files=['benchmarks/gpu-demo/native/retained.mjs','benchmarks/gpu-demo/native/transport.mjs','benchmarks/gpu-demo/native/gpu.mjs','benchmarks/gpu-demo/native/Host.swift','benchmarks/gpu-demo/native/raster.metal','benchmarks/gpu-demo/retained.mjs','benchmarks/gpu-demo/retained-study.mjs','benchmarks/gpu-demo/raster.mjs','benchmarks/gpu-demo/measure.mjs','benchmarks/gpu-demo/svg.mjs','benchmarks/gpu-demo/engine/src/main.rs','benchmarks/gpu-demo/engine/src/cache.rs','crates/pagura-paint/src/prepare/gpu_raster.rs','crates/pagura-paint/src/prepare/gpu_raster/shared.rs','crates/pagura-raster/src/band/gpu.rs'];
const hashes=Object.fromEntries(await Promise.all(files.map(async p=>[p,await sha256(await readFile(resolve(root,p)))])));
const binaryHash=await sha256(await readFile(binary)),metalHash=await sha256(await readFile(metalBinary));
const engine=new Host(binary),host=new Host(metalBinary,[resolve(root,'benchmarks/gpu-demo/native/raster.metal')],true);
const cases=[],failures=[];
try{
  const capability=(await host.ready).meta,gpu=new MetalBackend(host,capability);
  const config={samples:Number(option('--samples','100')),warmups:Number(option('--warmups','5'))},dpis=option('--dpis','72,150,300').split(',').map(Number);
  const inputs=[];for(let i=2;i<process.argv.length;i++)if(process.argv[i]==='--pdf')inputs.push({label:`Document ${inputs.length+1}`,bytes:await readFile(resolve(process.argv[++i]))});
  if(process.argv.includes('--fixtures')){
    const browser=JSON.parse(await readFile(resolve(option('--browser-report','target/gpu-demo/retained-fixtures.json')))),paths=new Map();
    for(const file of await readdir(resolve(root,'corpus/manifests')))if(file.endsWith('.json'))for(const c of JSON.parse(await readFile(resolve(root,'corpus/manifests',file))).cases)paths.set(c.id,c);
    for(const entry of browser.cases){
      const fixture=paths.get(entry.id);await engine.open(await readFile(resolve(root,fixture.path)));
      const options={page:0,dpi:fixture.dpi??72},packet=await engine.render(8,options);
      if(packet.meta.fallback){if(!entry.fallback)failures.push({id:entry.id,error:'Native shared admission differs'});continue;}
      if(entry.fallback){failures.push({id:entry.id,error:'Native shared admission differs'});continue;}
      const job=await gpu.prepare(packet.data);try{
        const cpu=await engine.render(7,options),out=await job.render(null);
        if(await sha256(packet.data)!==entry.packet_sha256||await sha256(cpu.data)!==entry.cpu_sha256)failures.push({id:entry.id,error:'Native/WASM shared packet or CPU mismatch'});
        cases.push({id:entry.id,accuracy:pixelDifference(out.pixels,cpu.data)});
      }finally{await job.destroy();}
    }
  }
  for(const input of inputs){
    const input_sha256=await sha256(input.bytes);await engine.open(input.bytes);
    for(const dpi of dpis){
      console.log(`Retained native ${input.label} ${dpi} DPI`);
      const out=await compareRetained(engine,gpu,{page:0,dpi},{...config,setupPhase:cases.length});Object.assign(out.result,{label:input.label,input_sha256});cases.push(out.result);
      if(dpi===150&&!process.argv.includes('--skip-zoom'))out.result.zoom=await zoomRetained(engine,gpu,{page:0,dpi});
    }
  }
  const unchanged=(await Promise.all(files.map(async p=>await sha256(await readFile(resolve(root,p)))===hashes[p]))).every(Boolean)&&binaryHash===await sha256(await readFile(binary))&&metalHash===await sha256(await readFile(metalBinary));
  if(!unchanged)failures.push({error:'Sources changed while running'});
  const report={timestamp:new Date().toISOString(),capability,cases,failures,build:{commit:execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim(),source_hashes:hashes,engine_sha256:binaryHash,metal_sha256:metalHash,sources_unchanged:unchanged}};
  await writeFile(resolve(option('--output','target/gpu-demo/retained-native.json')),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify({cases:cases.length,failures,capability}));if(failures.length)process.exitCode=1;
}finally{engine.close();host.close();}
