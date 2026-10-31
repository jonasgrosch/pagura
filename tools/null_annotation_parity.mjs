// Independent authored pixels and structural policy on native/SIMD/scalar.
import assert from 'node:assert/strict';
import {existsSync, readFileSync, mkdtempSync, rmSync, readdirSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {WASI} from 'node:wasi';

if (process.argv[2] === '--wasi') {
  const [,,,module,input,output,mode,dpi,operation,page] = process.argv;
  const wasi = new WASI({version:'preview1', returnOnExit:true,
    args:['null-annotations',input,output,mode,dpi,operation,page],
    preopens:{'/source':resolve('.'),'/scratch':process.env.NULL_ANNOTATION_SCRATCH}});
  const instance = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(module)),
    {wasi_snapshot_preview1:wasi.wasiImport});
  process.exitCode = wasi.start(instance);
} else {
  const root = resolve('.');
  const scratch = mkdtempSync(join(tmpdir(),'pagura-null-annotations-'));
  const selection = JSON.parse(readFileSync('benchmarks/probes/null-annotation-fixtures.json'));
  const catalog = new Map(readdirSync('corpus/manifests').filter(p=>p.endsWith('.json')).flatMap(
    p=>JSON.parse(readFileSync(join('corpus/manifests',p))).cases.map(c=>[c.id,c])));
  assert.equal(selection.cases.length,59);
  assert.equal(new Set(selection.cases).size,59,'duplicate selection');
  const binaries = [
    ['native',['target/release/examples/null_annotation_parity']],
    ['simd',[process.execPath,'--wasm-max-mem-pages=8192',fileURLToPath(import.meta.url),'--wasi',
      'target/wasm32-wasip1/release/examples/null_annotation_parity.wasm']],
    ['scalar',[process.execPath,'--wasm-max-mem-pages=8192',fileURLToPath(import.meta.url),'--wasi',
      'target/scalar/wasm32-wasip1/release/examples/null_annotation_parity.wasm']],
  ];
  const digest = path=>createHash('sha256').update(readFileSync(path)).digest('hex');
  function verified(record) {
    assert.equal(digest(record.path),record.sha256,`${record.path}: changed input/oracle`);
    return record.path;
  }
  function ppm(bytes) {
    const h=/^P6\s+(\d+)\s+(\d+)\s+255\s/.exec(bytes.subarray(0,80).toString('ascii'));
    assert(h,'complete P6 header required');
    const width=Number(h[1]),height=Number(h[2]),pixels=bytes.subarray(h[0].length);
    assert.equal(pixels.length,width*height*3,'complete RGB raster required');
    return {width,height,pixels};
  }
  function compare(output,expected,tolerance) {
    const a=ppm(readFileSync(output)),b=ppm(readFileSync(expected));
    assert.equal(a.width,b.width);assert.equal(a.height,b.height);
    for(let i=0;i<a.pixels.length;i++) assert(Math.abs(a.pixels[i]-b.pixels[i])<=tolerance,`oracle channel${i}`);
  }
  function invoke(c,name,command,mode,operation,inputPath) {
    const stem=`${c.id.replaceAll('/','-')}-${name}-${mode}-${operation}`;
    const localOutput=join(scratch,stem);
    const input=name==='native'?resolve(inputPath):`/source/${inputPath}`;
    const output=name==='native'?localOutput:`/scratch/${stem}`;
    const result=spawnSync(command[0],[...command.slice(1),input,output,mode,String(c.dpi??72),operation,String(c.page??0)],
      {cwd:root,encoding:'utf8',timeout:60000,env:{...process.env,NULL_ANNOTATION_SCRATCH:scratch}});
    assert(!result.error,`${stem}: ${result.error}`);
    return {result,localOutput,stem};
  }
  let pdfs=0,policies=0,links=0;
  const errors=new Map();
  try {
    for(const id of selection.cases) {
      const c=catalog.get(id);assert(c,`${id}: missing case`);
      if(c.kind==='pdf') {
        pdfs++;
        verified({path:c.path,sha256:c.sha256});
        if(c.expected==='render') verified({path:c.oracle,sha256:c.oracle_sha256});
      } else {
        assert.equal(c.kind,'vector');assert(['annotation-null-signature-policy','annotation-null-reference-signature-policy'].includes(c.driver));
        policies++;verified(c.files['.pdf']);verified(c.files['.outcome']);
      }
      for(const [name,command] of binaries) for(const mode of ['strict','repair']) {
        if(c.kind==='vector') {
          const {result:r,localOutput,stem}=invoke(c,name,command,mode,'policy',c.files['.pdf'].path);
          assert.equal(r.status,0,`${stem}: ${r.stderr}`);
          assert(r.stdout.split('\n').includes('original-source-preserved'),`${stem}: source identity`);
          assert.deepEqual(readFileSync(localOutput),readFileSync(c.files['.outcome'].path),`${stem}: structural policy`);
          continue;
        }
        const {result:r,localOutput,stem}=invoke(c,name,command,mode,'render',c.path);
        assert(r.stdout.split('\n').includes('original-source-preserved'),`${stem}: source identity on all outcomes`);
        if(process.env.PAGURA_QUAL_SOURCE_REVISION) assert(r.stdout.split('\n').includes('qualification-source='+process.env.PAGURA_QUAL_SOURCE_REVISION));
        assert(r.stdout.split('\n').includes('qualification-page='+String(c.page??0)));
        if(c.page>0) assert(r.stdout.split('\n').includes('same-context-prior-page-rendered=0'));
        const rejectedRepair=mode==='strict' && !c.strict_valid && c.repairs.length>0;
        if(c.expected==='render' && !rejectedRepair) {
          assert.equal(r.status,0,`${stem}: ${r.stderr}`);
          assert(r.stdout.split('\n').includes('original-source-preserved'),`${stem}: source identity`);
          for(const tag of c.repairs) assert(r.stdout.split('\n').includes(tag),`${stem}: missing${tag}`);
          compare(localOutput,c.oracle,c.tolerance??0);
          const gold=ppm(readFileSync(c.oracle)),white=readFileSync(localOutput+'.rgba'),transparent=readFileSync(localOutput+'.transparent.rgba');
          assert.equal(white.length,gold.width*gold.height*4);assert.equal(transparent.length,white.length);
          for(let pixel=0;pixel<gold.width*gold.height;pixel++) {
            const color=[...gold.pixels.subarray(pixel*3,pixel*3+3)];assert(color.every(v=>v===0||v===255));
            assert.deepEqual([...white.subarray(pixel*4,pixel*4+4)],[...color,255]);
            assert.deepEqual([...transparent.subarray(pixel*4,pixel*4+4)],color.every(v=>v===255)?[0,0,0,0]:[...color,255]);
          }
          const q=invoke(c,name,command,mode,'links',c.path);
          assert.equal(q.result.status,0,`${q.stem}: ${q.result.stderr}`);
          assert(q.result.stdout.split('\n').includes('link-identity-preserved'),`${q.stem}: link identity`);
          assert(q.result.stdout.split('\n').includes('original-source-preserved'),`${q.stem}: source identity`);
          assert.equal(readFileSync(q.localOutput,'utf8'),`${c.expected_link_count}\n`,`${q.stem}: all links`);
          links++;
        } else {
          assert.equal(r.status,1,`${stem}: expected refusal: ${r.stderr}`);
          for(const suffix of ['', '.rgba','.transparent.rgba']) assert(!existsSync(localOutput+suffix),`${stem}: partial output after refusal`);
          const errorLines=r.stderr.split('\n').filter(x=>x.startsWith('Error:'));assert.equal(errorLines.length,1);
          const key=c.id+'-'+mode;if(name==='native')errors.set(key,errorLines[0]);assert.equal(errorLines[0],errors.get(key));
          const contract=c.mode_errors?.[mode];
          if(contract) {
            assert.equal(r.stderr.match(/kind:\s*([A-Za-z]+)/)?.[1],contract.kind,`${stem}: explicit mode kind`);
            if(Object.hasOwn(contract,'repair')) assert(r.stderr.includes(contract.repair===null?'repair: None':'repair: Some(Repair('+JSON.stringify(contract.repair)+'))'),`${stem}: original strict repair contract`);
            if(contract.message!==undefined) assert(r.stderr.includes('message: '+JSON.stringify(contract.message)),`${stem}: original strict message`);
          }
          else if(rejectedRepair) assert(c.repairs.some(tag=>r.stderr.includes(tag)),`${stem}: strict repair tag`);
          else {
            const kind={syntax:'Syntax',limit:'Limit',cycle:'Cycle','invalid-argument':'InvalidArgument'}[c.render_error?.toLowerCase()];
            assert(kind,`${stem}: explicit refusal kind required`);
            assert.equal(r.stderr.match(/kind:\s*([A-Za-z]+)/)?.[1],kind,`${stem}: error kind`);
          }
        }
      }
    }
  } finally {rmSync(scratch,{recursive:true,force:true});}
  assert.equal(pdfs,38);assert.equal(policies,21);assert.equal(links,60);
  console.log('Null annotation controls passed:38 PDFs and21 structural-policy vectors ×native/SIMD/scalar ×Strict/Repair;60 independent link-identity checks. Complete RGB/white and transparent RGBA; actual page1 prefill in the same context. Placeholder signatures assert no integrity.');
}
