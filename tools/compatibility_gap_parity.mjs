// Independent fixture oracles on native, SIMD WASI and scalar WASI.
import assert from 'node:assert/strict';
import { existsSync, readFileSync, mkdtempSync, rmSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { WASI } from 'node:wasi';

if (process.argv[2] === '--wasi') {
  const [, , , module, input, output, mode, dpi] = process.argv;
  const wasi = new WASI({version:'preview1', returnOnExit:true,
    args:['compatibility-gap', input, output, mode, dpi],
    preopens:{'/source':resolve('.'), '/scratch':process.env.COMPATIBILITY_GAP_SCRATCH}});
  const instance = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(module)),
    {wasi_snapshot_preview1:wasi.wasiImport});
  process.exitCode = wasi.start(instance);
} else {
  const root = resolve('.');
  const scratch = mkdtempSync(join(tmpdir(), 'pagura-compatibility-gaps-'));
  const selection = JSON.parse(readFileSync('benchmarks/probes/compatibility-gap-fixtures.json'));
  const catalog = new Map(readdirSync('corpus/manifests').filter(p => p.endsWith('.json')).flatMap(
    p => JSON.parse(readFileSync(join('corpus/manifests',p))).cases.map(c => [c.id,c])));
  assert(selection.cases.length > 0, 'missing compatibility fixtures');
  assert.equal(new Set(selection.cases).size,selection.cases.length,'duplicate fixture selection');
  const binaries = [
    ['native', ['target/release/examples/compatibility_gap_parity']],
    ['simd', [process.execPath, fileURLToPath(import.meta.url), '--wasi',
      'target/wasm32-wasip1/release/examples/compatibility_gap_parity.wasm']],
    ['scalar', [process.execPath, fileURLToPath(import.meta.url), '--wasi',
      'target/scalar/wasm32-wasip1/release/examples/compatibility_gap_parity.wasm']],
  ];
  function ppm(bytes) {
    const header = /^P6\s+(\d+)\s+(\d+)\s+255\s/.exec(bytes.subarray(0,80).toString('ascii'));
    assert(header,'P6 oracle header required');
    const width = Number(header[1]), height = Number(header[2]);
    const pixels = bytes.subarray(header[0].length);
    assert.equal(pixels.length,width*height*3,'complete RGB raster required');
    return {width,height,pixels};
  }
  function compare(output,expected,tolerance) {
    const a=ppm(readFileSync(output)), b=ppm(readFileSync(expected));
    assert.equal(a.width,b.width); assert.equal(a.height,b.height);
    for(let i=0;i<a.pixels.length;i++) {
      assert(Math.abs(a.pixels[i]-b.pixels[i])<=tolerance,`oracle channel ${i}`);
    }
  }
  try {
    for (const id of selection.cases) {
      const c = catalog.get(id);
      assert(c?.kind === 'pdf',`${id}: missing PDF fixture`);
      for (const [name,command] of binaries) {
        for (const mode of ['strict','repair']) {
          const stem = `${id.replaceAll('/','-')}-${name}-${mode}.ppm`;
          const localOutput=join(scratch,stem);
          const input=name==='native'?resolve(c.path):`/source/${c.path}`;
          const output=name==='native'?localOutput:`/scratch/${stem}`;
          const r=spawnSync(command[0],[...command.slice(1),input,output,mode,String(c.dpi??72)],
            {cwd:root,encoding:'utf8',timeout:60000,env:{...process.env,COMPATIBILITY_GAP_SCRATCH:scratch}});
          assert(!r.error,`${stem}: ${r.error}`);
          const rejectedRepair=mode==='strict' && !c.strict_valid && c.repairs.length>0;
          if(c.expected==='render' && !rejectedRepair) {
            assert.equal(r.status,0,`${stem}: ${r.stderr}`);
            for(const tag of c.repairs) assert(r.stdout.split('\n').includes(tag),`${stem}: missing ${tag}`);
            compare(localOutput,c.oracle,c.tolerance??0);
          } else {
            assert.equal(r.status,1,`${stem}: expected refusal: ${r.stderr}`);
            assert(!existsSync(localOutput),`${stem}: partial output after refusal`);
            if(rejectedRepair) assert(c.repairs.some(tag=>r.stderr.includes(tag)),`${stem}: strict repair tag`);
            else if(c.expected==='unsupported') assert(r.stderr.includes(c.feature),`${stem}: feature tag`);
            else assert(r.stderr.includes(c.render_error==='limit'?'Limit':'Syntax'),`${stem}: error kind`);
          }
        }
      }
    }
  } finally { rmSync(scratch,{recursive:true,force:true}); }
  console.log(`Compatibility controls passed: ${selection.cases.length} fixtures × native/SIMD/scalar × strict/repair`);
}
