// Independent full RGB/RGBA and exact diagnostic/error parity for wrapper recovery.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {existsSync,mkdtempSync,readFileSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {WASI} from 'node:wasi';

if(process.argv[2]==='--wasi') {
  const [,,,module,input,output,mode]=process.argv;
  const wasi=new WASI({version:'preview1',returnOnExit:true,args:['jbig2-wrapper',input,output,mode],
    preopens:{'/source':resolve('.'),'/scratch':process.env.JBIG2_WRAPPER_SCRATCH}});
  const instance=await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(module)),
    {wasi_snapshot_preview1:wasi.wasiImport});
  process.exitCode=wasi.start(instance);
} else {
  assert(process.argv.slice(2).every(arg=>arg==='--native-only'),'unknown parity argument');
  const nativeOnly=process.argv.includes('--native-only');
  const source='corpus/manifests/recovery_jbig2_wrapper.json';
  const manifest=JSON.parse(readFileSync(source));
  const cases=manifest.cases.filter(c=>c.kind==='pdf');
  const vectors=new Map(manifest.cases.filter(c=>c.kind==='vector').map(c=>[c.id.split('/').at(-1),c]));
  assert.equal(cases.length,36);assert.equal(cases.filter(c=>c.expected==='render').length,13);
  const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
  function checked(path,digest){const bytes=readFileSync(path);assert.equal(hash(bytes),digest,path);return bytes;}
  const suffix=process.platform==='win32'?'.exe':'';
  const commands=[['native',[process.env.JBIG2_WRAPPER_NATIVE||`target/release/examples/jbig2_wrapper_parity${suffix}`]],
    ['simd',[process.execPath,fileURLToPath(import.meta.url),'--wasi','target/wasm32-wasip1/release/examples/jbig2_wrapper_parity.wasm']],
    ['scalar',[process.execPath,fileURLToPath(import.meta.url),'--wasi','target/scalar/wasm32-wasip1/release/examples/jbig2_wrapper_parity.wasm']]];
  const selected=nativeOnly?commands.slice(0,1):commands;
  const modules=Object.fromEntries(selected.map(([name,command])=>{const path=name==='native'?command[0]:command.at(-1);return [name,{path,sha256:hash(readFileSync(path))}];}));
  const scratch=mkdtempSync(join(tmpdir(),'pagura-jbig2-wrapper-'));const reference=new Map();const rows=[];
  try {
    for(const c of cases) {
      checked(c.path,c.sha256);
      let expected;
      if(c.expected==='render') {
        const rgb=checked(c.oracle,c.oracle_sha256),rgba=vectors.get('rgba-'+c.id.split('/').at(-1));assert(rgba);
        expected={ppm:rgb,rgba:checked(rgba.files['.white'].path,rgba.files['.white'].sha256),
          transparent:checked(rgba.files['.clear'].path,rgba.files['.clear'].sha256)};
      }
      for(const [variant,command] of selected)for(const mode of ['strict','repair']) {
        const stem=`${c.id.replaceAll('/','-')}-${variant}-${mode}`,host=join(scratch,stem+'.ppm');
        const input=variant==='native'?resolve(c.path):'/source/'+c.path;
        const output=variant==='native'?host:'/scratch/'+stem+'.ppm';
        const result=spawnSync(command[0],[...command.slice(1),input,output,mode],
          {cwd:resolve('.'),encoding:'utf8',timeout:30000,env:{...process.env,JBIG2_WRAPPER_SCRATCH:scratch}});
        assert(!result.error&&result.signal===null,`${stem}: timeout/signal`);
        const renders=c.expected==='render'&&(mode==='repair'||c.strict_valid),key=`${c.id}:${mode}`;
        const row={id:c.id,variant,mode,exit:result.status,input_sha256:c.sha256};
        if(!renders) {
          assert.equal(result.status,1,`${stem}: expected refusal: ${result.stderr}`);
          assert([host,host+'.rgba',host+'.transparent.rgba'].every(path=>!existsSync(path)),`${stem}: partial output`);
          const errors=result.stderr.split('\n').filter(line=>line.startsWith('Error:'));assert.equal(errors.length,1);
          assert(errors[0].includes('kind: Syntax'),`${stem}: expected syntax refusal`);
          if(mode==='strict'&&!c.strict_valid)assert(errors[0].includes('jbig2.file-wrapper'),`${stem}: missing strict repair identity`);
          if(variant==='native')reference.set(key,errors[0]);else assert.equal(errors[0],reference.get(key),`${stem}: exact error parity`);
          row.error=errors[0];
        } else {
          assert.equal(result.status,0,`${stem}: ${result.stderr}`);
          const diagnostics=result.stdout.trim().split('\n').filter(Boolean);
          assert.deepEqual(diagnostics.map(line=>line.split(' ')[1]),c.repairs,`${stem}: exact repair list`);
          for(const line of diagnostics)assert(line.startsWith('DIAGNOSTIC ')&&line.includes('number: 5, generation: 0')&&line.includes('Some(ObjectId'),`${stem}: source context`);
          if(c.repairs.length)assert(diagnostics[0].includes('offset: 0 }'),`${stem}: original header offset`);
          if(c.repairs.includes('jbig2.eof-page-association')) {
            const raw=vectors.get(c.native_case).files['.jb2'];const bytes=checked(raw.path,raw.sha256);
            const trailing=c.native_case==='native-long-page-associations'?8:5;
            assert(diagnostics[1].includes(`offset: ${bytes.length-trailing} }`),`${stem}: original EOF offset`);
          }
          const bytes={ppm:readFileSync(host),rgba:readFileSync(host+'.rgba'),transparent:readFileSync(host+'.transparent.rgba')};
          for(const channel of ['ppm','rgba','transparent'])assert.deepEqual(bytes[channel],expected[channel],`${stem}: independent ${channel}`);
          if(variant==='native')reference.set(key,{bytes,diagnostics});
          else {
            const before=reference.get(key);assert.deepEqual(diagnostics,before.diagnostics,`${stem}: diagnostic parity`);
            for(const channel of ['ppm','rgba','transparent'])assert.deepEqual(bytes[channel],before.bytes[channel],`${stem}: full output parity`);
          }
          row.outputs=Object.fromEntries(Object.entries(bytes).map(([key,value])=>[key,hash(value)]));row.diagnostics=diagnostics;
          row.maximum_independent_channel_error=0;
        }
        rows.push(row);
      }
    }
  } finally {rmSync(scratch,{recursive:true,force:true});}
  assert.equal(rows.length,72*selected.length);assert.equal(rows.filter(row=>row.exit===0).length,16*selected.length);
  const revision=spawnSync('git',['rev-parse','HEAD'],{encoding:'utf8'});assert.equal(revision.status,0);
  writeFileSync(nativeOnly?'jbig2-wrapper-parity-native-results.json':'jbig2-wrapper-parity-results.json',JSON.stringify({complete:true,variants:selected.map(([name])=>name),modules,source_revision:revision.stdout.trim(),
    manifest_sha256:hash(readFileSync(source)),rows,scope:'Independent full RGB/whiteRGBA/transparentRGBA; original source positions; native/SIMD/scalar strict/repair; explicit refusals and no partial output. No public-file admission or benchmark claim.'},null,2)+'\n');
  console.log(`JBIG2 wrapper parity PASS: 36 inputs, ${rows.length} rows, ${16*selected.length} renders, ${48*selected.length} independent output comparisons`);
}
