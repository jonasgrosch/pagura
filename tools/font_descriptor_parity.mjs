// Complete source-owned descriptor contracts on native/SIMD/scalar.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, mkdirSync, rmSync, readdirSync, openSync, writeSync, fsyncSync, closeSync, statfsSync } from 'node:fs';
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
  assert.equal(selection.cases.length,77);
  assert.equal(new Set(selection.cases).size,77);
  assert(selection.cases.slice(0,64).every(id=>id.startsWith('recovery_font_descriptor/')));
  assert.deepEqual(selection.cases.slice(64),JSON.parse(readFileSync('corpus/manifests/selected_font_interactions.json')).cases.map(c=>c.id));
  const root=resolve('.'),evidence=resolve('work/font-descriptor-parity-evidence'),scratch=join(evidence,'outputs');
  assert(!existsSync(evidence),'evidence directory must be fresh');
  mkdirSync(resolve('work'),{recursive:true});
  mkdirSync(evidence);mkdirSync(scratch);
  const hash=p=>createHash('sha256').update(readFileSync(p)).digest('hex');
  const target=process.env.CARGO_TARGET_DIR??'target';
  const binaries=[['native',[join(target,'release/examples/font_descriptor_parity')]],
    ['simd',[process.execPath,'--wasm-max-mem-pages=8192',fileURLToPath(import.meta.url),'--wasi',join(target,'wasm32-wasip1/release/examples/font_descriptor_parity.wasm')]],
    ['scalar',[process.execPath,'--wasm-max-mem-pages=8192',fileURLToPath(import.meta.url),'--wasi',join(target,'scalar/wasm32-wasip1/release/examples/font_descriptor_parity.wasm')]]];
  function ppm(path) {
    const data=readFileSync(path),head=/^P6\n(\d+) (\d+)\n255\n/.exec(data.subarray(0,80).toString('ascii'));
    assert(head);const width=Number(head[1]),height=Number(head[2]),pixels=data.subarray(head[0].length);
    assert.equal(pixels.length,width*height*3);return {width,height,pixels};
  }
  const error=r=>{const rows=r.stderr.split('\n').filter(x=>x.startsWith('Error:'));assert.equal(rows.length,1);return rows[0];};
  function journal(record) {
    const fd=openSync(join(evidence,'process-results.jsonl'),'a');
    try {writeSync(fd,JSON.stringify(record)+'\n');fsyncSync(fd);} finally {closeSync(fd);}
    const directory=openSync(evidence,'r');
    try {fsyncSync(directory);} finally {closeSync(directory);}
  }
  function mixedStderr(result,variant) {
    const lines=result.stderr.split('\n');
    const warning=lines.findIndex(line=>line===`(node:${result.pid}) ExperimentalWarning: WASI is an experimental feature and might change at any time`);
    if(warning!==-1) {
      assert.notEqual(variant,'native');
      assert.equal(lines[warning+1],'(Use `node --trace-warnings ...` to show where the warning was created)');
      lines.splice(warning,2);
    }
    const residual=lines.filter(line=>line!=='');
    if(result.status===0) assert.deepEqual(residual,[]);
    else {assert.equal(residual.length,1);assert.match(residual[0],/^Error: Error \{ kind: /);}
  }
  function mixedRgba(gold,white,transparent,tolerance) {
    for(let pixel=0;pixel<gold.width*gold.height;pixel++) {
      const color=[...gold.pixels.subarray(pixel*3,pixel*3+3)];
      const green=color[0]===0&&color[1]===255&&color[2]===0;
      assert(green||color.every(v=>v===color[0]),'frozen black/green source grammar');
      const expectedWhite=[...color,255],expectedTransparent=green?[0,255,0,255]:[0,0,0,255-color[0]];
      for(let channel=0;channel<4;channel++) {
        const bound=green?0:tolerance;
        assert(Math.abs(white[pixel*4+channel]-expectedWhite[channel])<=(channel===3?0:bound));
        assert(Math.abs(transparent[pixel*4+channel]-expectedTransparent[channel])<=(channel===3?bound:0));
      }
    }
  }
  function priorPixels(out,requested,baseline,variant) {
    const targets=[out+'.prior0.ppm',out+'.prior0.rgba',out+'.prior0.transparent.rgba'];
    if(!requested) {for(const path of targets) assert(!existsSync(path));return;}
    const image=ppm(targets[0]);assert.equal(image.width,64);assert.equal(image.height,64);
    for(let i=0;i<image.pixels.length;i++) assert.equal(image.pixels[i],i%3===2?255:0);
    for(const path of targets.slice(1)) {
      const data=readFileSync(path);assert.equal(data.length,64*64*4);
      for(let i=0;i<data.length;i++) assert.equal(data[i],i%4>=2?255:0);
    }
    const actual=targets.map(hash);if(variant==='native') baseline.set('prior0',actual);
    assert.deepEqual(actual,baseline.get('prior0'));
  }
  function mixedGeometry(stdout,contract) {
    const rows=stdout.split('\n');assert(rows.includes('text-glyph-count=3'));
    assert.deepEqual(rows.filter(x=>x.startsWith('text-unicode=')).map(x=>x.slice(13)),['H','A','A']);
    const positions=rows.filter(x=>x.startsWith('text-position=')).map(x=>JSON.parse(x.slice(14)));
    assert.equal(positions.length,3);assert.notEqual(positions[0][0],positions[1][0]);assert.equal(positions[1][0],positions[2][0]);
    assert.deepEqual(positions.map(p=>p.slice(1)),[[3,8,3+contract.cid_advance,8],[40,4,50,4],[50,4,60,4]]);
    const quads=rows.filter(x=>x.startsWith('text-quad=')).map(x=>JSON.parse(x.slice(10)));
    assert.equal(quads.length,3);assert.deepEqual(quads.slice(1),contract.alias_quads);
  }
  let completed=false;
  let checked=0;
  journal({event:'plan',source_revision:process.env.PAGURA_QUAL_SOURCE_REVISION??'local-unqualified',selector_sha256:hash('benchmarks/probes/font-descriptor-fixtures.json'),cases:selection.cases,processes:462});
  try {
    for(const id of selection.cases) {
      const c=catalog.get(id);assert.equal(c?.kind,'pdf');assert.equal(hash(c.path),c.sha256);
      const mixed=id.startsWith('selected_font_interactions/');
      if(c.oracle) assert.equal(hash(c.oracle),c.oracle_sha256);
      const baseline=new Map();
      for(const [variant,command] of binaries) for(const mode of ['strict','repair']) {
        const stem=id.replaceAll('/','-')+'-'+variant+'-'+mode+'.ppm',out=join(scratch,stem);
        const targets=[out,out+'.rgba',out+'.transparent.rgba'];
        const text=c.selected_font_geometry?'geometry':c.text_geometry_contract?'text':'paint';
        const prior0=mixed&&mode==='repair'&&c.page===1;
        const args=[variant==='native'?resolve(c.path):'/source/'+c.path,
          variant==='native'?out:'/scratch/'+stem,mode,String(c.dpi??72),String(c.page??0),text,...(prior0?['prior0']:[])];
        const succeeds=c.expected==='render'&&(mode==='repair'||c.strict_valid);
        const free=statfsSync(root);assert(free.bavail*free.bsize>=3*1024**3,'3GiB required before each actual process');
        journal({event:'pending',id,variant,mode,command:[...command,...args],input_sha256:c.sha256,page:c.page??0,dpi:c.dpi??72,expected_status:succeeds?0:1});
        const r=spawnSync(command[0],[...command.slice(1),...args],{cwd:root,encoding:'utf8',timeout:60000,
          env:{...process.env,FONT_DESCRIPTOR_SCRATCH:scratch}});
        journal({event:'completed',id,variant,mode,pid:r.pid,status:r.status,signal:r.signal,error:r.error?.message,stdout:r.stdout,stderr:r.stderr});
        const files=[...targets,out+'.prior0.ppm',out+'.prior0.rgba',out+'.prior0.transparent.rgba'];
        journal({event:'output-hashes',id,variant,mode,files:Object.fromEntries(files.map(path=>[path,existsSync(path)?hash(path):null]))});
        if(mixed) {
          assert.equal(hash(c.path),c.sha256);
          mixedStderr(r,variant);
          assert.equal(r.stdout.split('\n').includes('prior-page-rendered-dropped=0'),prior0);
          priorPixels(out,prior0,baseline,variant);
        }
        assert(!r.error,`${id}/${variant}/${mode}: ${r.error}`);
        if(process.env.PAGURA_QUAL_SOURCE_REVISION) assert(r.stdout.split('\n').includes('qualification-source='+process.env.PAGURA_QUAL_SOURCE_REVISION));
        assert(r.stdout.split('\n').includes('qualification-page='+String(c.page??0)));
        assert(r.stdout.split('\n').includes('source-bytes-preserved=pass'));
        if(succeeds) {
          assert.equal(r.status,0,`${id}/${variant}/${mode}: ${r.stderr}`);
          const image=ppm(out),gold=ppm(c.oracle);assert.equal(image.width,gold.width);assert.equal(image.height,gold.height);
          for(let i=0;i<image.pixels.length;i++) assert(Math.abs(image.pixels[i]-gold.pixels[i])<=(c.tolerance??0),`${id}: channel${i}`);
          const white=readFileSync(targets[1]),transparent=readFileSync(targets[2]);
          assert.equal(white.length,image.width*image.height*4);
          assert.equal(transparent.length,image.width*image.height*4);
          // The frozen H/green witnesses contain only opaque integer-grid
          // fills; white in their independent RGB oracle is clear background.
          if(mixed) mixedRgba(gold,white,transparent,c.tolerance??0);
          else for(let pixel=0;pixel<image.width*image.height;pixel++) {
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
          if(c.selected_font_geometry) mixedGeometry(r.stdout,c.selected_font_geometry);
        } else {
          assert.equal(r.status,1,`${id}/${variant}/${mode}: expected refusal`);
          for(const target of targets) assert(!existsSync(target),`${id}: partial target output`);
          const actual=error(r);if(variant==='native')baseline.set(mode,actual);assert.equal(actual,baseline.get(mode));
          const contract=c.mode_errors?.[mode];
          if(contract) {
            assert(actual.includes('kind: '+contract.kind));
            if(contract.message!==undefined) assert(actual.includes('message: '+JSON.stringify(contract.message)));
            if(mixed&&contract.feature===null) assert(!actual.includes('Feature('));
            else if(contract.feature!==undefined) assert(actual.includes('Feature('+JSON.stringify(contract.feature)+')'));
            if(contract.repair!==undefined) assert(actual.includes(contract.repair===null?'repair: None':'repair: Some(Repair('+JSON.stringify(contract.repair)+'))'));
          } else if(c.expected==='unsupported') assert(actual.includes(c.feature));
          else assert(actual.includes('kind: '+c.render_error[0].toUpperCase()+c.render_error.slice(1)));
          if(mixed) {
            const complete=mode==='strict'?(selection.mixed_strict_errors[id]??selection.mixed_additional_errors[id]?.strict):selection.mixed_additional_errors[id]?.repair;
            assert.equal(typeof complete,'string','every mixed refusal needs a complete independent Error');
            assert.equal(actual,complete);
            if(!contract) {assert(actual.includes('kind: Unsupported(Feature('+JSON.stringify(c.feature)+'))'));assert(actual.includes('repair: None'));}
          }
        }
        if(mode==='repair') for(const tag of c.repairs) assert(r.stdout.split('\n').includes(tag),`${id}: missing repair${tag}`);
        if(mixed&&mode==='repair') for(const tag of c.information??[]) assert(r.stdout.split('\n').includes(tag),`${id}: missing information${tag}`);
        journal({event:'checks-passed',id,variant,mode,expected_status:succeeds?0:1,full_rgb_white_transparent:succeeds,complete_mixed_error:mixed&&!succeeds,prior0,geometry:!!c.selected_font_geometry&&succeeds,source_sha_after:mixed?hash(c.path):null});
        checked++;
        // Hashes and raw results survive; successful images are not archived
        // by this CI smoke. A failing request retains its actual output files.
        for(const path of files) if(existsSync(path)) rmSync(path);
      }
    }
    assert.equal(checked,462);
    journal({event:'complete',checked,passed:true});
    console.log('Font descriptor controls passed:77 ×native/SIMD/scalar ×Strict/Repair =462');
    completed=true;
  } finally {
    if(completed) rmSync(scratch,{recursive:true,force:true});
    else console.error('Raw journal, passed checks and failure outputs retained at '+evidence);
  }
}
