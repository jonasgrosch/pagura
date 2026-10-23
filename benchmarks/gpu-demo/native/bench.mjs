/** Native Metal + native Rust reference; Node standard library only. */
import {spawn, execFileSync} from 'node:child_process';
import {readFile, writeFile, readdir} from 'node:fs/promises';
import {resolve, dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {validateRaster} from '../raster.mjs';
import {pixelDifference} from '../svg.mjs';
import {stats, hash} from '../measure.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const option = (k,d) => process.argv.includes(k) ? process.argv[process.argv.indexOf(k)+1] : d;
const digest = b => createHash('sha256').update(b).digest('hex');
class Host {
  constructor(binary, args = [], hello = false) {
    this.chunks = [];this.length = 0;this.pending = null;
    this.ready = hello ? new Promise((resolve,reject) => {this.pending={resolve,reject};}) : Promise.resolve();
    this.child=spawn(binary,args,{stdio:['pipe','pipe','inherit']});
    this.child.stdout.on('data',b=>{this.chunks.push(b);this.length+=b.length;this.receive();});
    this.child.on('error',e=>this.pending?.reject(e));
    this.child.on('exit',c=>this.pending?.reject(new Error(`Native host exit ${c}`)));
  }
  receive(){
    if(this.length<4)return;const header=Buffer.alloc(4);let offset=0;for(const chunk of this.chunks){const count=Math.min(4-offset,chunk.length);chunk.copy(header,offset,0,count);offset+=count;if(offset===4)break;}const n=header.readUInt32LE();
    if(n>68*1024*1024){this.pending?.reject(new Error('Native frame budget'));this.close();return;}
    if(this.length<n+4)return;const data=Buffer.concat(this.chunks);const pending=this.pending;this.pending=null;this.chunks=[];this.length=0;
    try{if(!pending||data.length!==n+4)throw new Error('Unexpected native frame');const size=data.readUInt32LE(4);if(size>n-4)throw new Error('Native metadata budget');const meta=JSON.parse(data.subarray(8,8+size));if(!meta.ok)throw new Error(meta.error);pending.resolve({meta,data:data.subarray(8+size)});}catch(e){pending?.reject(e);}
  }
  request(bytes){return new Promise((resolve,reject)=>{if(this.pending){reject(new Error('Concurrent native requests'));return;}this.pending={resolve,reject};const h=Buffer.alloc(4);h.writeUInt32LE(bytes.length);this.child.stdin.write(h);this.child.stdin.write(bytes);});}
  open(pdf){const b=Buffer.alloc(pdf.length+4);b.writeUInt32LE(1);b.set(pdf,4);return this.request(b);}
  render(op,dpi){const b=Buffer.alloc(32);b.writeUInt32LE(op);b.writeUInt32LE(dpi,8);b.writeUInt32LE(32,12);return this.request(b);}
  close(){this.child.stdin.end();this.child.kill('SIGTERM');}
}
const enginePath=resolve(option('--binary','target/gpu-demo/release/pagura-gpu-demo'));
const metalPath=resolve(option('--metal','target/gpu-demo/metal-host'));
const shaderPath=resolve(root,'benchmarks/gpu-demo/native/raster.metal');
const files=['benchmarks/gpu-demo/native/bench.mjs','benchmarks/gpu-demo/native/Host.swift','benchmarks/gpu-demo/native/raster.metal', 'crates/pagura-paint/src/prepare/gpu_raster.rs','crates/pagura-raster/src/band/gpu.rs'];
const hashes=Object.fromEntries(await Promise.all(files.map(async p=>[p,digest(await readFile(resolve(root,p)))])));
const binaryHash=digest(await readFile(enginePath)),metalHash=digest(await readFile(metalPath));
const engine=new Host(enginePath),metal=new Host(metalPath,[shaderPath],true);
const failures=[],cases=[];
try {
  const capability=(await metal.ready).meta;
  const count=Number(option('--samples','100')),warmups=Number(option('--warmups','5')),dpis=option('--dpis','72,150,300').split(',').map(Number);
  if(!Number.isInteger(count)||count<1||count>1000||!Number.isInteger(warmups)||warmups<0||warmups>100)throw new Error('Invalid sample count');
  const inputs=[];
  for(let i=2;i<process.argv.length;i++)if(process.argv[i]==='--pdf')inputs.push({label:`Document ${inputs.length+1}`,bytes:await readFile(resolve(process.argv[++i]))});
  if(process.argv.includes('--fixtures')){
    const browser=JSON.parse(await readFile(resolve(option('--browser-report','target/gpu-demo/coverage-fixtures.json'))));
    const paths=new Map();for(const file of await readdir(resolve(root,'corpus/manifests')))if(file.endsWith('.json'))for(const c of JSON.parse(await readFile(resolve(root,'corpus/manifests',file))).cases)paths.set(c.id,c);
    for(const entry of browser.cases){
      const c=paths.get(entry.id);await engine.open(await readFile(resolve(root,c.path)));const packet=await engine.render(5,c.dpi??72);
      if(packet.meta.fallback){failures.push({id:entry.id,error:'Native admission differs'});continue;}
      validateRaster(packet.data);const cpu=await engine.render(2,c.dpi??72),output=await metal.request(packet.data);
      if(digest(packet.data)!==entry.packet_sha256||digest(cpu.data)!==entry.cpu_sha256)failures.push({id:entry.id,error:'Native/WASM packet or CPU parity'});
      cases.push({id:entry.id,accuracy:pixelDifference(output.data,cpu.data),webgpu_delta:entry.accuracy,packet_hash:hash(packet.data),pixel_hash:hash(output.data)});
    }
  }
  for(const input of inputs){
    await engine.open(input.bytes);
    for(const dpi of dpis){
      const reference=await engine.render(2,dpi),jobs={},rows=[],last={cpu:reference.data},first={cpu:reference.meta};
      for(const [mode,op]of[['local',5],['absolute',6]]){const p=await engine.render(op,dpi);first[mode]={...p.meta,packet_sha256:p.meta.fallback?null:digest(p.data)};if(!p.meta.fallback){validateRaster(p.data);jobs[mode]=true;}}
      const modes=['cpu',...Object.keys(jobs)];
      for(let i=-warmups;i<count;i++){
        const order=modes.slice((i+warmups)%modes.length).concat(modes.slice(0,(i+warmups)%modes.length));if(Math.floor((i+warmups)/modes.length)%2)order.reverse();const row={};
        for(const mode of order){const started=performance.now(),p=await engine.render(mode==='cpu'?2:mode==='local'?5:6,dpi);const output=mode==='cpu'?{data:p.data,meta:{complete_ms:0,kernel_ms:null,allocation_ms:0}}:await metal.request(p.data);
          last[mode]=output.data;row[mode]={prepare_ms:p.meta.prepare_ms,export_ms:p.meta.execute_ms,engine_ms:p.meta.engine_ms,...output.meta,render_ms:p.meta.engine_ms+output.meta.complete_ms+output.meta.allocation_ms,frame_ms:performance.now()-started};
          if(mode==='cpu'&&digest(p.data)!==digest(reference.data))throw new Error('Native CPU repeat differs');
        }if(i>=0)rows.push(row);
      }
      const entry={label:input.label,dpi,input_sha256:digest(input.bytes),samples:count,warmups,first,raw:rows,modes:{}};
      for(const mode of ['cpu','local','absolute']){
        if(mode!=='cpu'&&!jobs[mode]){entry.modes[mode]={fallback:first[mode].fallback};continue;}
        entry.modes[mode]=Object.fromEntries(['prepare_ms','export_ms','engine_ms','complete_ms','kernel_ms','render_ms','frame_ms'].filter(k=>rows[0][mode][k]!==null).map(k=>[k,stats(rows.map(r=>r[mode][k]))]));
        entry.modes[mode].accuracy=pixelDifference(last[mode],reference.data);entry.modes[mode].pixel_hash=hash(last[mode]);entry.modes[mode].pixel_sha256=digest(last[mode]);
      }cases.push(entry);console.log(input.label,dpi,Object.fromEntries(Object.entries(entry.modes).map(([k,v])=>[k,v.fallback??v.render_ms.median])));
    }
  }
  const unchanged=(await Promise.all(files.map(async p=>digest(await readFile(resolve(root,p)))===hashes[p]))).every(Boolean)&&binaryHash===digest(await readFile(enginePath))&&metalHash===digest(await readFile(metalPath));
  if(!unchanged)failures.push({error:'Sources changed while running'});
  const report={timestamp:new Date().toISOString(),capability,cases,failures,build:{commit:execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim(),engine_sha256:binaryHash,metal_sha256:metalHash,source_hashes:hashes,sources_unchanged:unchanged}};
  await writeFile(resolve(option('--output','target/gpu-demo/native-coverage.json')),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify({capability,cases:cases.length,failures}));if(failures.length)process.exitCode=1;
}finally{engine.close();metal.close();}
