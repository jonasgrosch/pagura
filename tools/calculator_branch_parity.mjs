// Independent full pixels and separately labelled public Function API refusals.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, mkdtempSync, rmSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { WASI } from 'node:wasi';

if (process.argv[2] === '--wasi') {
  const module = process.argv[3];
  const wasi = new WASI({version:'preview1', returnOnExit:true,
    args:['calculator-branch', ...process.argv.slice(4)],
    preopens:{'/source':resolve('.'), '/scratch':process.env.CALCULATOR_BRANCH_SCRATCH}});
  const instance = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(module)),
    {wasi_snapshot_preview1:wasi.wasiImport});
  process.exitCode = wasi.start(instance);
} else {
  const root=resolve('.'), scratch=mkdtempSync(join(tmpdir(),'pagura-calculator-branch-'));
  const selection=JSON.parse(readFileSync('benchmarks/probes/calculator-branch-fixtures.json'));
  const catalog=new Map(readdirSync('corpus/manifests').filter(p=>p.endsWith('.json')).flatMap(
    p=>JSON.parse(readFileSync(join('corpus/manifests',p))).cases.map(c=>[c.id,c])));
  const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
  const checked=(path,digest)=>{
    const bytes=readFileSync(path); assert.equal(sha(bytes),digest,path); return bytes;
  };
  const binaries=[
    ['native',['target/release/examples/calculator_branch_parity']],
    ['simd',[process.execPath,fileURLToPath(import.meta.url),'--wasi',
      'target/wasm32-wasip1/release/examples/calculator_branch_parity.wasm']],
    ['scalar',[process.execPath,fileURLToPath(import.meta.url),'--wasi',
      'target/scalar/wasm32-wasip1/release/examples/calculator_branch_parity.wasm']],
  ];
  const run=(command,args)=>{
    const result=spawnSync(command[0],[...command.slice(1),...args],{
      cwd:root,encoding:'utf8',timeout:120000,
      env:{...process.env,CALCULATOR_BRANCH_SCRATCH:scratch}});
    assert(!result.error,String(result.error));return result;
  };
  const error=result=>{
    const lines=result.stderr.split(/\r?\n/).filter(line=>line.startsWith('Error:'));
    assert.equal(lines.length,1,result.stderr);return lines[0];
  };
  const ppm=bytes=>{
    const header=/^P6\s+(\d+)\s+(\d+)\s+255\s/.exec(bytes.subarray(0,80).toString('ascii'));
    assert(header,'complete independent P6 oracle required');
    const width=Number(header[1]),height=Number(header[2]),pixels=bytes.subarray(header[0].length);
    assert.equal(pixels.length,width*height*3);return {width,height,pixels};
  };
  const scaled=(bytes,factor)=>{
    const original=ppm(bytes),width=original.width*factor,height=original.height*factor;
    const rgb=Buffer.alloc(width*height*3),rgba=Buffer.alloc(width*height*4);
    for(let y=0;y<height;y++) for(let x=0;x<width;x++) {
      const source=(Math.floor(y/factor)*original.width+Math.floor(x/factor))*3;
      const pixel=y*width+x;original.pixels.copy(rgb,pixel*3,source,source+3);
      original.pixels.copy(rgba,pixel*4,source,source+3);rgba[pixel*4+3]=255;
    }
    return {ppm:Buffer.concat([Buffer.from(`P6\n${width} ${height}\n255\n`),rgb]),rgba};
  };
  const native=new Map();let rows=0;
  assert(selection.cases.length>0 && selection.vectors.length===3);
  assert.equal(new Set([...selection.cases,...selection.vectors]).size,
    selection.cases.length+selection.vectors.length,'duplicate selection');
  try {
    for(const id of selection.cases) {
      const c=catalog.get(id);assert.equal(c?.kind,'pdf',id);checked(c.path,c.sha256);
      const targets=[{page:0,oracle:c.oracle,oracle_sha256:c.oracle_sha256},...(c.qualification_pages??[])];
      for(const target of targets) for(const dpi of [72,144]) {
        let expected;
        if(c.expected==='render') expected=scaled(checked(target.oracle,target.oracle_sha256),dpi/72);
        for(const [variant,command] of binaries) for(const mode of ['strict','repair']) {
          const key=`${id}-p${target.page}-${dpi}-${mode}`,stem=`${key.replaceAll('/','-')}-${variant}.ppm`;
          const local=join(scratch,stem),input=variant==='native'?resolve(c.path):`/source/${c.path}`;
          const output=variant==='native'?local:`/scratch/${stem}`;
          const result=run(command,['pdf',input,output,mode,String(dpi),String(target.page)]);
          checked(c.path,c.sha256);
          if(target.page>0) assert(result.stdout.split('\n').includes('prior-page-rendered=0'));
          if(c.expected==='render') {
            assert.equal(result.status,0,result.stderr);
            assert.deepEqual(readFileSync(local),expected.ppm,key);
            assert.deepEqual(readFileSync(local+'.rgba'),expected.rgba,key);
            assert.deepEqual(readFileSync(local+'.transparent.rgba'),expected.rgba,key);
            const value=[sha(readFileSync(local)),sha(readFileSync(local+'.rgba')),
              sha(readFileSync(local+'.transparent.rgba'))];
            if(variant==='native') native.set(key,value);else assert.deepEqual(value,native.get(key));
          } else {
            assert.equal(result.status,1,result.stderr);
            for(const path of [local,local+'.rgba',local+'.transparent.rgba']) assert(!existsSync(path));
            const actual=error(result);
            if(c.expected==='unsupported') assert(actual.includes(c.feature),actual);
            else {
              const wanted={syntax:'Syntax',limit:'Limit',cycle:'Cycle'}[c.render_error?.toLowerCase()];
              assert(wanted && actual.includes(wanted),actual);
            }
            if(variant==='native') native.set(key,actual);else assert.equal(actual,native.get(key));
          }
          rows++;
        }
      }
    }
    for(const id of selection.vectors) {
      const c=catalog.get(id);assert.equal(c?.kind,'vector');assert.equal(c.expected,'unsupported');
      assert.deepEqual(c.domain,[[0,1]]);assert(c.range.every(pair=>pair[0]===0 && pair[1]===1));
      const file=c.files['.ps'];checked(file.path,file.sha256);
      for(const [variant,command] of binaries) {
        const input=variant==='native'?resolve(file.path):`/source/${file.path}`;
        const result=run(command,['function',input,String(c.range.length)]);checked(file.path,file.sha256);
        assert.equal(result.status,1,result.stderr);const actual=error(result);
        assert(actual.includes('Unsupported') && actual.includes(c.feature),actual);
        if(variant==='native') native.set(id,actual);else assert.equal(actual,native.get(id));rows++;
      }
    }
  } finally {rmSync(scratch,{recursive:true,force:true});}
  console.log(`Calculator branch contracts passed: ${rows} PDF/API process rows; full independent RGB/RGBA, native/SIMD/scalar and complete refusal parity`);
}
