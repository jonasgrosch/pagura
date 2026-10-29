// Native, SIMD WASI and scalar WASI must independently match geometric oracles.
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { WASI } from 'node:wasi';

if (process.argv[2] === '--wasi') {
  const [, , , module, input, output, mode] = process.argv;
  const wasi = new WASI({version:'preview1', args:['image-repair', input, output, mode],
    preopens:{'/source':resolve('.'), '/scratch':process.env.IMAGE_REPAIR_SCRATCH}});
  const instance = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(module)),
    {wasi_snapshot_preview1:wasi.wasiImport});
  wasi.start(instance);
} else {
  const root = resolve('.');
  const scratch = mkdtempSync(join(tmpdir(), 'pagura-image-repairs-'));
  const entries = JSON.parse(readFileSync('corpus/manifests/recovery.json')).cases.filter(
    c => c.expected === 'render' && c.repairs.some(t => ['jbig2.page-capabilities','jpx.raw-codestream'].includes(t)));
  assert(entries.length > 0, 'missing image repair fixtures');
  const binaries = [
    ['native', ['target/release/examples/image_repair_parity']],
    ['simd', [process.execPath, fileURLToPath(import.meta.url), '--wasi',
      'target/wasm32-wasip1/release/examples/image_repair_parity.wasm']],
    ['scalar', [process.execPath, fileURLToPath(import.meta.url), '--wasi',
      'target/scalar/wasm32-wasip1/release/examples/image_repair_parity.wasm']],
  ];
  try {
    for (const c of entries) {
      for (const [name, command] of binaries) {
        const stem = `${c.id.replaceAll('/','-')}-${name}`;
        const input = name === 'native' ? resolve(c.path) : `/source/${c.path}`;
        const output = name === 'native' ? join(scratch,stem+'.ppm') : `/scratch/${stem}.ppm`;
        for (const mode of ['strict','repair']) {
          const result = spawnSync(command[0], [...command.slice(1),input,output,mode],
            {cwd:root, encoding:'utf8',timeout:30000,env:{...process.env,IMAGE_REPAIR_SCRATCH:scratch}});
          assert(!result.error, `${stem}: ${result.error}`);
          if (mode === 'strict') {
            assert.equal(result.status,1,`${stem}: strict rejection: ${result.stderr}`);
            assert(c.repairs.some(tag => result.stderr.includes(tag)),`${stem}: missing strict repair tag`);
          } else {
            assert.equal(result.status,0,`${stem}: ${result.stderr}`);
            for (const tag of c.repairs) assert(result.stdout.includes(tag),`${stem}: missing ${tag}`);
            assert.deepEqual(readFileSync(join(scratch,stem+'.ppm')),readFileSync(c.oracle),`${stem}: analytic pixels`);
          }
        }
      }
    }
  } finally { rmSync(scratch,{recursive:true,force:true}); }
  console.log(`Image repair controls passed: ${entries.length} fixtures × native/SIMD/scalar × strict/repair`);
}
