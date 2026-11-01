// Complete source-owned descriptor contracts on native/SIMD/scalar.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, mkdtempSync, rmSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { WASI } from 'node:wasi';

if (process.argv[2] === '--wasi') {
  const [, , , module, ...args] = process.argv;
  const wasi = new WASI({version:'preview1', returnOnExit:true,
    args:['font-descriptor', ...args],
    preopens:{'/source':resolve('.'), '/scratch':process.env.FONT_DESCRIPTOR_SCRATCH}});
  const instance = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(module)),
    {wasi_snapshot_preview1:wasi.wasiImport});
  process.exitCode = wasi.start(instance);
} else {
  const selection = JSON.parse(readFileSync('benchmarks/probes/font-descriptor-fixtures.json'));
  const catalog = new Map(readdirSync('corpus/manifests').filter(p=>p.endsWith('.json')).flatMap(
    p=>JSON.parse(readFileSync(join('corpus/manifests',p))).cases.map(c=>[c.id,c])));
  assert.equal(selection.cases.length,64);
  assert.equal(new Set(selection.cases).size,64);
  const root=resolve('.'),scratch=mkdtempSync(join(tmpdir(),'pagura-font-descriptor-'));
  const hash=p=>createHash('sha256').update(readFileSync(p)).digest('hex');
  const binaries=[['native',['target/release/examples/font_descriptor_parity']],
    ['simd',[process.execPath,'--wasm-max-mem-pages=8192',fileURLToPath(import.meta.url),'--wasi','target/wasm32-wasip1/release/examples/font_descriptor_parity.wasm']],
    ['scalar',[process.execPath,'--wasm-max-mem-pages=8192',fileURLToPath(import.meta.url),'--wasi','target/scalar/wasm32-wasip1/release/examples/font_descriptor_parity.wasm']]];
  function ppm(path) {
    const data=readFileSync(path),head=/^P6\n(\d+) (\d+)\n255\n/.exec(data.subarray(0,80).toString('ascii'));
    assert(head);const width=Number(head[1]),height=Number(head[2]),pixels=data.subarray(head[0].length);
    assert.equal(pixels.length,width*height*3);return {width,height,pixels};
  }
  const error=r=>{const rows=r.stderr.split('\n').filter(x=>x.startsWith('Error:'));assert.equal(rows.length,1);return rows[0];};
  try {
    for(const id of selection.cases) {
      const c=catalog.get(id);assert.equal(c?.kind,'pdf');assert.equal(hash(c.path),c.sha256);
      if(c.oracle) assert.equal(hash(c.oracle),c.oracle_sha256);
      const baseline=new Map();
      for(const [variant,command] of binaries) for(const mode of ['strict','repair']) {
        const stem=id.replaceAll('/','-')+'-'+variant+'-'+mode+'.ppm',out=join(scratch,stem);
        const targets=[out,out+'.rgba',out+'.transparent.rgba'];
        const text=c.text_geometry_contract?'text':'paint';
        const args=[variant==='native'?resolve(c.path):'/source/'+c.path,
          variant==='native'?out:'/scratch/'+stem,mode,String(c.dpi??72),String(c.page??0),text];
        const r=spawnSync(command[0],[...command.slice(1),...args],{cwd:root,encoding:'utf8',timeout:60000,
          env:{...process.env,FONT_DESCRIPTOR_SCRATCH:scratch}});
        assert(!r.error,`${id}/${variant}/${mode}: ${r.error}`);
        if(process.env.PAGURA_QUAL_SOURCE_REVISION) assert(r.stdout.split('\n').includes('qualification-source='+process.env.PAGURA_QUAL_SOURCE_REVISION));
        assert(r.stdout.split('\n').includes('qualification-page='+String(c.page??0)));
        assert(r.stdout.split('\n').includes('source-bytes-preserved=pass'));
        const succeeds=c.expected==='render'&&(mode==='repair'||c.strict_valid);
        if(succeeds) {
          assert.equal(r.status,0,`${id}/${variant}/${mode}: ${r.stderr}`);
          const image=ppm(out),gold=ppm(c.oracle);assert.equal(image.width,gold.width);assert.equal(image.height,gold.height);
          for(let i=0;i<image.pixels.length;i++) assert(Math.abs(image.pixels[i]-gold.pixels[i])<=(c.tolerance??0),`${id}: channel${i}`);
          const white=readFileSync(targets[1]),transparent=readFileSync(targets[2]);
          assert.equal(white.length,image.width*image.height*4);
          assert.equal(transparent.length,image.width*image.height*4);
          // The frozen H/green witnesses contain only opaque integer-grid
          // fills; white in their independent RGB oracle is clear background.
          for(let pixel=0;pixel<image.width*image.height;pixel++) {
            const color=[...gold.pixels.subarray(pixel*3,pixel*3+3)];
            assert(color.every(v=>v===0||v===255),'integer opaque oracle required');
            const clear=color.every(v=>v===255);
            assert.deepEqual([...white.subarray(pixel*4,pixel*4+4)],[...color,255]);
            assert.deepEqual([...transparent.subarray(pixel*4,pixel*4+4)],clear?[0,0,0,0]:[...color,255]);
          }
          const actual=targets.map(hash);if(variant==='native')baseline.set(mode,actual);assert.deepEqual(actual,baseline.get(mode));
          if(c.text_geometry_contract) {
            const contract=c.text_geometry_contract;
            if(contract.error_kind) assert(r.stdout.split('\n').some(line=>line.startsWith('text-error=Error { kind: '+contract.error_kind)));
            else {
              assert(r.stdout.split('\n').includes('text-glyph-count=2'));
              const actual=r.stdout.split('\n').filter(x=>x.startsWith('text-quad=')).map(x=>JSON.parse(x.slice('text-quad='.length)));
              assert.deepEqual(actual,contract.quads);
            }
          }
        } else {
          assert.equal(r.status,1,`${id}/${variant}/${mode}: expected refusal`);
          for(const target of targets) assert(!existsSync(target),`${id}: partial target output`);
          const actual=error(r);if(variant==='native')baseline.set(mode,actual);assert.equal(actual,baseline.get(mode));
          const contract=c.mode_errors?.[mode];
          if(contract) {
            assert(actual.includes('kind: '+contract.kind));
            if(contract.message!==undefined) assert(actual.includes('message: '+JSON.stringify(contract.message)));
            if(contract.feature!==undefined) assert(actual.includes('Feature('+JSON.stringify(contract.feature)+')'));
            if(contract.repair!==undefined) assert(actual.includes(contract.repair===null?'repair: None':'repair: Some(Repair('+JSON.stringify(contract.repair)+'))'));
          } else if(c.expected==='unsupported') assert(actual.includes(c.feature));
          else assert(actual.includes('kind: '+c.render_error[0].toUpperCase()+c.render_error.slice(1)));
        }
        if(mode==='repair') for(const tag of c.repairs) assert(r.stdout.split('\n').includes(tag),`${id}: missing repair${tag}`);
      }
    }
    console.log('Font descriptor controls passed:64 ×native/SIMD/scalar ×Strict/Repair');
  } finally {rmSync(scratch,{recursive:true,force:true});}
}
