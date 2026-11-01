/** Native display comparison: actual CAMetalLayer presentation, no emulated window. */
import {readFile,writeFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {resolve,dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {Host} from './transport.mjs';
import {MetalBackend,NativePresentation} from './gpu.mjs';
import {DisplayStudy} from '../display-study.mjs';
import {sha256} from '../retained-study.mjs';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'../../..');
const option=(key,value)=>process.argv.includes(key)?process.argv[process.argv.indexOf(key)+1]:value;
const binary=resolve(option('--binary','target/gpu-demo/release/pagura-gpu-demo')),metal=resolve(option('--metal','target/gpu-demo/metal-host'));
const files=['benchmarks/gpu-demo/native/display.mjs','benchmarks/gpu-demo/native/transport.mjs','benchmarks/gpu-demo/native/gpu.mjs','benchmarks/gpu-demo/native/Host.swift','benchmarks/gpu-demo/native/Presentation.swift','benchmarks/gpu-demo/native/World.swift','benchmarks/gpu-demo/native/raster.metal','benchmarks/gpu-demo/native/area.metal','benchmarks/gpu-demo/native/world.metal','benchmarks/gpu-demo/native/present.metal','benchmarks/gpu-demo/world.mjs','benchmarks/gpu-demo/present.mjs','benchmarks/gpu-demo/display-study.mjs','benchmarks/gpu-demo/retained.mjs','benchmarks/gpu-demo/retained-study.mjs','benchmarks/gpu-demo/raster.mjs','benchmarks/gpu-demo/measure.mjs','benchmarks/gpu-demo/svg.mjs','benchmarks/gpu-demo/engine/src/main.rs','benchmarks/gpu-demo/engine/src/world.rs','benchmarks/gpu-demo/engine/src/cache.rs','crates/pagura-paint/src/prepare/gpu_world.rs','crates/pagura-paint/src/prepare/gpu_world/geometry.rs','crates/pagura-paint/src/prepare/gpu_world/patches.rs','crates/pagura-paint/src/plan.rs','crates/pagura-paint/src/prepare/planner/runs.rs','crates/pagura-raster/src/band/gpu.rs','benchmarks/gpu-demo/policy.json'];
const hashes=Object.fromEntries(await Promise.all(files.map(async file=>[file,await sha256(await readFile(resolve(root,file)))])));
const engineHash=await sha256(await readFile(binary)),metalHash=await sha256(await readFile(metal)),commit=execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim();
const engine=new Host(binary),host=new Host(metal,[resolve(root,'benchmarks/gpu-demo/native/raster.metal')],true),cases=[],failures=[];
try {
 const info=(await host.ready).meta,gpu=new MetalBackend(host,info),presenter=new NativePresentation(gpu);
 const harness={presenter,clock:'Native completion/presentation-or-discard callback plus pipe transport; presentation counts and reported drawable times are separate',activate:()=>{},cpu:(...args)=>presenter.cpu(...args),complete:async()=>{},captureCpu:async(...args)=>{presenter.captureNext=true;await presenter.cpu(...args);return presenter.capture();}};
 const args={samples:Number(option('--samples','100')),warmups:Number(option('--warmups','5'))};
 const inputs=[];for(let i=2;i<process.argv.length;i++)if(process.argv[i]==='--pdf')inputs.push({label:`Document ${inputs.length+1}`,bytes:await readFile(resolve(process.argv[++i]))});
 for(const input of inputs){
  const input_sha256=await sha256(input.bytes);await engine.open(input.bytes);const study=new DisplayStudy(engine,gpu,harness);
  try {for(const dpi of option('--dpis','72,150,300').split(',').map(Number)){
   console.log('Native display',input.label,dpi);const out=await study.compare({page:0,dpi},args);Object.assign(out,{label:input.label,input_sha256});cases.push(out);
   if(dpi===150&&!process.argv.includes('--skip-zoom')){console.log('Native novel/revisited zoom',input.label);out.zoom=await study.zoom({page:0,dpi});}
  }}finally{await study.clear();}
 }
 const unchanged=(await Promise.all(files.map(async file=>await sha256(await readFile(resolve(root,file)))===hashes[file]))).every(Boolean)&&engineHash===await sha256(await readFile(binary))&&metalHash===await sha256(await readFile(metal));
 if(!unchanged)failures.push({error:'Native display sources or binaries changed during run'});
 const report={timestamp:new Date().toISOString(),capability:info,cases,failures,build:{commit,source_hashes:hashes,engine_sha256:engineHash,metal_sha256:metalHash,sources_unchanged:unchanged}};
 await writeFile(resolve(option('--output','target/gpu-demo/display-native.json')),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify({cases:cases.length,failures}));if(failures.length)process.exitCode=1;
}finally{engine.close();host.close();}
