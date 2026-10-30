// Hosted observations on pinned public inputs. Corpus bytes/pixels stay temporary.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFileSync,writeFileSync,mkdtempSync,rmSync,existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {resolve,join} from 'node:path';
import {spawnSync} from 'node:child_process';

const repairs = new Set(JSON.parse(readFileSync('corpus/manifests/recovery.json')).cases.flatMap(c=>c.repairs));
const cases = JSON.parse(readFileSync('benchmarks/probes/image-repair-cases.json')).cases.filter(c=>repairs.has(c.repair));
const scratch=mkdtempSync(join(tmpdir(),'pagura-repair-observe-'));
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
const commands=[['native',['target/release/examples/image_repair_parity']],
  ['simd',[process.execPath,'tools/image_repair_parity.mjs','--wasi','target/wasm32-wasip1/release/examples/image_repair_parity.wasm']],
  ['scalar',[process.execPath,'tools/image_repair_parity.mjs','--wasi','target/scalar/wasm32-wasip1/release/examples/image_repair_parity.wasm']]];
const results=[];
try {
  for (const [index,c] of cases.entries()) {
    assert(c.url.startsWith('https://raw.githubusercontent.com/mozilla/pdf.js/157d30e833e5afbd282db5b1280aae4843f55c0a/test/pdfs/'));
    const response=await fetch(c.url,{signal:AbortSignal.timeout(30000)});
    assert(response.ok,`${c.id}: HTTP ${response.status}`);
    const chunks=[];let size=0;
    for await (const chunk of response.body) {
      size+=chunk.length;
      assert(size<=8*1024*1024,`${c.id}: input budget`);
      chunks.push(chunk);
    }
    const bytes=Buffer.concat(chunks);assert.equal(hash(bytes),c.sha256,`${c.id}: source digest`);
    writeFileSync(join(scratch,`input-${index}.pdf`),bytes);
    const current=[];
    for (const [runtime,command] of commands) {
      const input=runtime==='native'?join(scratch,`input-${index}.pdf`):`/scratch/input-${index}.pdf`;
      const stem=`output-${index}-${runtime}.ppm`;
      const output=runtime==='native'?join(scratch,stem):`/scratch/${stem}`;
      const run=spawnSync(command[0],[...command.slice(1),input,output,'repair'],{
        encoding:'utf8',timeout:10000,env:{...process.env,IMAGE_REPAIR_SCRATCH:scratch}});
      assert(!run.error && run.signal===null,`${c.id}: ${runtime} timeout/signal: ${run.error??run.signal}`);
      const pixels=existsSync(join(scratch,stem))?readFileSync(join(scratch,stem)):null;
      assert(run.status===0 || pixels===null,`${c.id}: partial output after failure`);
      const row={id:c.id,sha256:c.sha256,runtime,exit_code:run.status,
        rasterized:run.status===0,pixel_sha256:pixels?hash(pixels):null,
        diagnostics:run.stdout.trim().split('\n').filter(Boolean),error:run.stderr.slice(0,2000)};
      results.push(row);current.push(row);
    }
    assert(current.every(r=>r.exit_code===current[0].exit_code && r.pixel_sha256===current[0].pixel_sha256),`${c.id}: runtime outcome/pixel disagreement`);
  }
} finally {rmSync(scratch,{recursive:true,force:true});}
const report={scope:'Public first-page outcomes only; successful rendering is not an accuracy claim',
  built_revision:spawnSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).stdout.trim(),cases:cases.length,results};
writeFileSync('image-repair-observations.json',JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify({cases:cases.length,native_rasterized:results.filter(r=>r.runtime==='native'&&r.rasterized).length}));
