// Native/SIMD/scalar controls compare independent RGB/white RGBA and explicit
// literal-matrix transparent RGBA where available. Other alpha is parity only.
import assert from 'node:assert/strict';
import {readFileSync,mkdtempSync,rmSync,existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {resolve,join} from 'node:path';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {WASI} from 'node:wasi';

if (process.argv[2] === '--wasi') {
  const [,,,module,input,output,mode] = process.argv;
  const wasi = new WASI({version:'preview1',returnOnExit:true,args:['image-repair',input,output,mode],
    preopens:{'/source':resolve('.'),'/scratch':process.env.IMAGE_REPAIR_SCRATCH}});
  const instance = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(module)),
    {wasi_snapshot_preview1:wasi.wasiImport});
  process.exitCode = wasi.start(instance);
} else {
  const root=resolve('.'),scratch=mkdtempSync(join(tmpdir(),'pagura-image-repairs-'));
  const previous=JSON.parse(readFileSync('corpus/manifests/recovery.json')).cases.filter(
    c=>c.expected==='render'&&c.repairs.some(t=>['jbig2.page-capabilities','jpx.raw-codestream'].includes(t)));
  const added=JSON.parse(readFileSync('corpus/manifests/recovery_jbig2_tables.json')).cases;
  const entries=[...previous,...added.filter(c=>c.kind==='pdf')];
  const vectors=new Map(added.filter(c=>c.kind==='vector').map(c=>[c.id.split('/').at(-1),c]));
  assert(previous.length>0&&vectors.size>0,'missing image controls');
  const binaries=[['native',['target/release/examples/image_repair_parity']],
    ['simd',[process.execPath,fileURLToPath(import.meta.url),'--wasi','target/wasm32-wasip1/release/examples/image_repair_parity.wasm']],
    ['scalar',[process.execPath,fileURLToPath(import.meta.url),'--wasi','target/scalar/wasm32-wasip1/release/examples/image_repair_parity.wasm']]];
  const reference=new Map();
  function expectedRGBA(c) {
    const ppm=readFileSync(c.oracle),header=Buffer.from(`P6\n${c.width} ${c.height}\n255\n`);
    assert(ppm.subarray(0,header.length).equals(header));
    const rgb=ppm.subarray(header.length),white=Buffer.alloc(c.width*c.height*4);
    assert.equal(rgb.length,c.width*c.height*3);
    for(let p=0;p<c.width*c.height;p++){rgb.copy(white,p*4,p*3,p*3+3);white[p*4+3]=255;}
    if(!c.native_case)return {white};
    const vector=vectors.get(c.native_case);assert(vector);
    const params=Object.fromEntries(readFileSync(vector.files['.params'].path,'utf8').trim().split('\n').map(line=>line.split('=')));
    const width=Number(params.width),height=Number(params.height),packed=readFileSync(vector.files['.out'].path);
    const stride=Math.ceil(width/8),transparent=Buffer.alloc(c.width*c.height*4);
    assert.equal(packed.length,stride*height);
    // Fixture transform is [width 0 0 height 2 2]; source row0 is at the top.
    for(let y=0;y<height;y++)for(let x=0;x<width;x++){
      const black=(packed[y*stride+(x>>3)]>>(7-(x&7)))&1;
      if(c.stencil&&!black)continue;
      const p=((c.height-2-height+y)*c.width+2+x)*4;
      transparent.set(c.stencil?[255,0,0,255]:[255*(1-black),255*(1-black),255*(1-black),255],p);
    }
    return {white,transparent};
  }
  try {
    for(const c of entries)for(const [name,command] of binaries)for(const mode of ['strict','repair']){
      const stem=`${c.id.replaceAll('/','-')}-${name}-${mode}`,host=join(scratch,stem+'.ppm');
      const input=name==='native'?resolve(c.path):`/source/${c.path}`,output=name==='native'?host:`/scratch/${stem}.ppm`;
      const result=spawnSync(command[0],[...command.slice(1),input,output,mode],
        {cwd:root,encoding:'utf8',timeout:30000,env:{...process.env,IMAGE_REPAIR_SCRATCH:scratch}});
      assert(!result.error&&result.signal===null,`${stem}: timeout/signal`);
      const renders=c.expected==='render'&&(mode==='repair'||c.strict_valid),key=`${c.id}:${mode}`;
      if(!renders){
        assert.equal(result.status,1,`${stem}: expected refusal: ${result.stderr}`);
        assert([host,host+'.rgba',host+'.transparent.rgba'].every(p=>!existsSync(p)),`${stem}: partial output`);
        const errors=result.stderr.split('\n').filter(line=>line.startsWith('Error:'));assert.equal(errors.length,1);
        if(c.expected==='render')assert(c.repairs.some(tag=>errors[0].includes(tag)),`${stem}: missing repair tag`);
        else assert(errors[0].includes('kind: Syntax'),`${stem}: malformed table accepted`);
        if(name==='native')reference.set(key,errors[0]);else assert.equal(errors[0],reference.get(key),`${stem}: error parity`);
      } else {
        assert.equal(result.status,0,`${stem}: ${result.stderr}`);
        assert.deepEqual(result.stdout.trim().split('\n').filter(Boolean),c.repairs,`${stem}: exact diagnostic list`);
        const ppm=readFileSync(host),rgba=readFileSync(host+'.rgba'),alpha=readFileSync(host+'.transparent.rgba');
        assert.deepEqual(ppm,readFileSync(c.oracle),`${stem}: independent RGB`);
        const expected=expectedRGBA(c);assert.deepEqual(rgba,expected.white,`${stem}: independent white RGBA`);
        if(expected.transparent)assert.deepEqual(alpha,expected.transparent,`${stem}: independent transparent RGBA`);
        if(name==='native')reference.set(key,[ppm,rgba,alpha]);
        else for(const [i,bytes] of [ppm,rgba,alpha].entries())assert.deepEqual(bytes,reference.get(key)[i],`${stem}: full byte parity`);
      }
    }
  } finally {rmSync(scratch,{recursive:true,force:true});}
  console.log(`Image repair controls passed: ${entries.length} fixtures × native/SIMD/scalar × strict/repair`);
}
