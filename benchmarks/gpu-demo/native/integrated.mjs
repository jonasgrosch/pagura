/** Launch once: all timed requests stay in the Swift application. */
import {readFile,writeFile} from 'node:fs/promises';
import {spawn,execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {resolve} from 'node:path';
const option=(key,fallback)=>process.argv.includes(key)?process.argv[process.argv.indexOf(key)+1]:fallback;
const host=resolve(option('--host','target/gpu-demo/shared-host')),engine=resolve(option('--engine','target/gpu-demo/release/pagura-gpu-demo'));
const sources=execFileSync('git',['ls-files','crates','benchmarks/gpu-demo','web'],{encoding:'utf8'}).trim().split('\n').filter(p=>/\.(rs|swift|metal|wgsl|mjs|toml|json)$/.test(p));
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
async function freeze(){return Object.fromEntries(await Promise.all([...sources,host,engine].map(async p=>[p,digest(await readFile(p))])));}
const initial=await freeze(),commit=execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim(),args=['--engine',engine,'--shaders',resolve('benchmarks/gpu-demo/native'),'--samples',option('--samples','100'),'--warmups',option('--warmups','5'),'--cold-samples',option('--cold-samples','20')];
for(let i=2;i<process.argv.length;i++)if(process.argv[i]==='--pdf')args.push('--pdf',resolve(process.argv[++i]));
const start=performance.now(),child=spawn(host,args,{stdio:['ignore','pipe','inherit']}),chunks=[];child.stdout.on('data',b=>chunks.push(b));
const code=await new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',resolve);});if(code!==0)throw new Error(`Shared host exited ${code}`);
const report=JSON.parse(Buffer.concat(chunks)),after=await freeze();report.build={commit,source_hashes:initial,sources_unchanged:JSON.stringify(initial)===JSON.stringify(after)};report.process_wall_ms=performance.now()-start;
if(!report.build.sources_unchanged)report.failures.push({error:'Integrated sources or binaries changed during timing'});
await writeFile(option('--output','target/gpu-demo/integrated-native.json'),JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify({cases:report.cases.length,failures:report.failures,summary:report.cases.map(c=>({label:c.label,mode:c.mode,opening:c.opening,zoom:c.novel_zoom,scroll:c.scroll,fallback:c.fallback}))},null,2));if(report.failures.length)process.exitCode=1;
