/** Coverage experiment: fresh geometry export/upload/completion on every sample. */
import {demo} from './app.mjs';
import {stats, assertPixels, hash} from './measure.mjs';
import {pixelDifference, differencePixels} from './svg.mjs';
import {validateRaster} from './raster.mjs';

const modes = {cpu: 2, spans: 3, local: 5, absolute: 6};
export async function compareCoverage(engine, gpu, options, {samples = 9, warmups = 2} = {}) {
  const reference = await engine.render(2, options);
  const initial = {}, jobs = {}, paths = ['cpu'], rows = [], last = {cpu: reference.data};
  const first = {cpu: reference.meta};
  try {
    for (const [mode, op] of Object.entries(modes).slice(1)) {
      initial[mode] = await engine.render(op, options);
      first[mode] = initial[mode].meta;
      if (!initial[mode].meta.fallback) {
        jobs[mode] = gpu.prepare(initial[mode].data);
        paths.push(mode);
      }
    }
    const render = async mode => {
      const packet = await engine.render(modes[mode], options);
      if (packet.meta.fallback) throw new Error('Coverage admission changed');
      const output = mode === 'cpu' ? {pixels: packet.data, wall_ms: 0, kernel_ms: null} : await jobs[mode].render(packet.data);
      if (mode === 'cpu' || mode === 'spans') assertPixels(output.pixels, reference.data, mode);
      last[mode] = output.pixels;
      return {engine_ms: packet.meta.engine_ms, prepare_ms: packet.meta.prepare_ms, export_ms: packet.meta.execute_ms,
        complete_ms: output.wall_ms, kernel_ms: output.kernel_ms, render_ms: packet.meta.engine_ms + output.wall_ms,
        frame_ms: packet.host_ms + output.wall_ms};
    };
    for (let i = 0; i < warmups; i++) for (const mode of paths) await render(mode);
    for (let i = 0; i < samples; i++) {
      const order = paths.slice(i % paths.length).concat(paths.slice(0, i % paths.length));
      if (Math.floor(i / paths.length) % 2) order.reverse();
      const row = {};
      for (const mode of order) row[mode] = await render(mode);
      rows.push(row);
    }
    const result = {options, width: reference.meta.width, height: reference.meta.height, samples, warmups, first, raw: rows, modes: {}};
    for (const mode of Object.keys(modes)) {
      const job = jobs[mode];
      if (mode !== 'cpu' && !job) { result.modes[mode] = {fallback: first[mode].fallback};continue; }
      const times = rows.map(row => row[mode]);
      result.modes[mode] = Object.fromEntries(Object.keys(times[0]).filter(k => times[0][k] !== null).map(k => [k, stats(times.map(t => t[k]))]));
      result.modes[mode].accuracy = pixelDifference(last[mode], reference.data);
      if (job) result.modes[mode].packet = {bytes: initial[mode].data.length, marks: job.spans, edges: job.geometry?.edges,
        correction_cells: job.geometry?.correction_cells, clip_runs: job.geometry?.clip_runs, references: job.references, setup_ms: job.setup_ms};
      result.modes[mode].pixel_hash = hash(last[mode]);
    }
    return {result, pixels: last};
  } finally { for (const job of Object.values(jobs)) job.destroy(); }
}

function png(pixels, width, height) {
  const canvas = document.createElement('canvas');canvas.width = width;canvas.height = height;
  canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(pixels), width, height), 0, 0);
  return canvas.toDataURL('image/png');
}
export async function run(config) {
  await demo.ready;
  const {engine, gpu} = demo;
  if (!gpu) throw new Error('Hardware GPU required');
  if (config.mode === 'coverage-test') return check(engine, gpu);
  const cases = [], artifacts = [];
  const inputs = await (await fetch('/benchmark-inputs.json')).json();
  for (const input of inputs) {
    const pdf = new Uint8Array(await (await fetch(input.path)).arrayBuffer());
    const digest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', pdf)), b => b.toString(16).padStart(2, '0')).join('');
    await engine.open(pdf);
    for (const dpi of config.dpis) {
      console.log(`Coverage ${input.label} at ${dpi} DPI`);
      const output = await compareCoverage(engine, gpu, {page: 0, dpi}, config);
      output.result.label = input.label;output.result.input_sha256 = digest;cases.push(output.result);
      if (dpi === 150 && output.pixels.local) {
        const prefix = input.label.toLowerCase().replaceAll(' ', '-');
        for (const mode of ['cpu', 'local', 'absolute']) artifacts.push({name: `${prefix}-${mode}.png`, data_url: png(output.pixels[mode], output.result.width, output.result.height)});
        artifacts.push({name: `${prefix}-local-difference.png`, data_url: png(differencePixels(output.pixels.local, output.pixels.cpu), output.result.width, output.result.height)});
      }
    }
  }
  return {adapter: gpu.info, cases, artifacts, failures: [], summary: cases.map(c => ({label: c.label, dpi: c.options.dpi, modes: c.modes}))};
}

// Independent polygon oracle: clip each contour to a unit cell, shoelace area.
function polygonArea(vertices, x, y) {
  let points = vertices.map(p => [...p]);
  for (const [axis, limit, sign] of [[0,x,1],[0,x+1,-1],[1,y,1],[1,y+1,-1]]) {
    const next = [];
    for (let i = 0; i < points.length; i++) {
      const a = points[i], b = points[(i + 1) % points.length], inside = (a[axis] - limit) * sign >= 0, other = (b[axis] - limit) * sign >= 0;
      if (inside) next.push(a);
      if (inside !== other) {const t = (limit - a[axis]) / (b[axis] - a[axis]);next.push(a.map((v,j) => v + (b[j] - v) * t));}
    }
    points = next;
  }
  let area = 0;
  for (let i = 0; i < points.length; i++) {const a = points[i], b = points[(i+1)%points.length];area += (a[0]-x)*(b[1]-y)-(b[0]-x)*(a[1]-y);}
  return Math.min(1, Math.abs(area) / 2);
}
function oraclePacket(vertices, width = 32, height = 16) {
  const marksAt = 24, edgesAt = 40, extrasAt = edgesAt + vertices.length * 8, binsAt = extrasAt, bins = Math.ceil(width/32)*Math.ceil(height/8), refsAt = binsAt + bins * 2;
  const words = new Uint32Array(refsAt + bins), floats = new Float32Array(words.buffer);
  words.set([0x32555047,2,width,height,Math.ceil(width/32),bins,0,0,marksAt,edgesAt,extrasAt,binsAt,refsAt,1,vertices.length,0,bins,words.length,0xffffffff,1,0,0,0,0]);
  words.set([0,0,width,height,0,vertices.length,0,0,0,0,0,0xffff0000,0,0,0,0],marksAt);
  vertices.forEach((a,i) => {const b = vertices[(i+1)%vertices.length], at = edgesAt+i*8;floats.set([...a,...b,...b],at+1);});
  for (let i = 0; i < bins; i++) words.set([i,i+1],binsAt+i*2);
  return new Uint8Array(words.buffer);
}
async function check(engine, gpu) {
  const failures = [], oracle = [], cases = [];
  let variants = 0;
  const polygons = [ [[0.25,0.5],[31.75,0.5],[31.75,15.5],[0.25,15.5]], [[1.125,1.875],[30.875,2.125],[15.375,15.25]], [[0,0],[32,16],[0,16]], [[8.5,0.25],[9.25,15.875],[7.75,15.125]] ];
  for (const vertices of polygons) {
    const bytes = oraclePacket(vertices), job = gpu.prepare(bytes);
    try {
      const {pixels} = await job.render();let max = 0;
      for (let y=0;y<16;y++) for(let x=0;x<32;x++) {const expected = 255 - Math.floor(polygonArea(vertices,x,y)*255+0.5+1e-9);max=Math.max(max,Math.abs(pixels[(y*32+x)*4]-expected));}
      oracle.push({pixels:512,max_channel_error:max});if(max>1) failures.push({error:'Independent polygon oracle',max});
      for (const mutate of [w=>w[17]++, w=>w[14]=0, w=>w[refsAtSafe(w)] = 1, w=>w[9]=0]) {
        const bad=bytes.slice(),w=new Uint32Array(bad.buffer);mutate(w);let refused=false;try{validateRaster(bad);}catch{refused=true;}if(!refused)failures.push({error:'Malformed geometry accepted'});
      }
    } finally {job.destroy();}
  }
  // True monotone quadratic: independent adaptive integration of horizontal
  // widths, rather than a port of the shader's x(t)y'(t) primitive.
  const curveBytes = oraclePacket([[0,0],[16,0],[0,16]]);
  const cw = new Uint32Array(curveBytes.buffer), cf = new Float32Array(curveBytes.buffer);
  const curveAt = cw[9] + 8;cw[curveAt] = 1;cf.set([16,0,16,16,0,16],curveAt+1);
  const curveJob = gpu.prepare(curveBytes);
  try {
    const output = await curveJob.render();let max = 0,worst=null;
    const integrate = (f,a,b,eps,depth=0) => {
      const m=(a+b)/2,fa=f(a),fm=f(m),fb=f(b),whole=(b-a)*(fa+4*fm+fb)/6;
      const l=(a+m)/2,r=(m+b)/2,parts=(m-a)*(fa+4*f(l)+fm)/6+(b-m)*(fm+4*f(r)+fb)/6;
      return depth>=20||(depth>=4&&Math.abs(parts-whole)<15*eps) ? parts+(parts-whole)/15 : integrate(f,a,m,eps/2,depth+1)+integrate(f,m,b,eps/2,depth+1);
    };
    for(let y=0;y<16;y++)for(let x=0;x<32;x++){
      const area=integrate(v=>{const t=1-Math.sqrt(Math.max(0,1-v/16));return Math.max(0,Math.min(1,16*(1-t*t)-x));},y,y+1,1e-10);
      const expected=255-Math.floor(area*255+0.5),actual=output.pixels[(y*32+x)*4],delta=Math.abs(actual-expected);if(delta>max){max=delta;worst={x,y,expected,actual,area};}
    }
    oracle.push({kind:'true-quadratic-independent-integral',pixels:512,max_channel_error:max,worst});if(max>1)failures.push({error:'Quadratic integral oracle',max});
  }finally{curveJob.destroy();}
  const precision=[];
  const vertices=[[0.1,0.1],[31.9,0.1],[31.9,15.9],[0.1,15.9]];
  for(const local of [true,false]){
    const bytes=oraclePacket(vertices),w=new Uint32Array(bytes.buffer),f=new Float32Array(bytes.buffer),origin=1048576;
    w[6]=origin;w[7]=origin;for(let i=0;i<4;i++)w[24+i]+=origin;
    w[32]=local?origin:0;w[33]=local?origin:0;w[19]=Number(local);
    if(!local)for(let i=w[9];i<w[10];i+=8)for(let j=1;j<=6;j++)f[i+j]+=origin;
    const job=gpu.prepare(bytes);try{const output=await job.render();let max=0;for(let y=0;y<16;y++)for(let x=0;x<32;x++){const expected=255-Math.floor(polygonArea(vertices,x,y)*255+0.5);max=Math.max(max,Math.abs(output.pixels[(y*32+x)*4]-expected));}precision.push({local,origin,max_channel_error:max});if(local&&max>1)failures.push({error:'Local precision stress',max});}finally{job.destroy();}
  }
  const fixtures = await (await fetch('/fixture-index.json')).json();
  let admitted = 0, fallback = 0;
  for (const fixture of fixtures) {
    try {
      await engine.open(new Uint8Array(await (await fetch(fixture.path)).arrayBuffer()));
      const options={page:0,dpi:fixture.dpi};const packet=await engine.render(5,options);
      if(packet.meta.fallback){fallback++;continue;}
      const cpu=await engine.render(2,options),job=gpu.prepare(packet.data);
      try {
        const result=await job.render();const accuracy=pixelDifference(result.pixels,cpu.data);
        const entry={id:fixture.id,accuracy,packet_hash:hash(packet.data),cpu_hash:hash(cpu.data)};
        if(admitted%4===0){
          const band=await engine.render(5,{...options,band:1});assertPixels(band.data,packet.data,'GPU geometry band independence');
          const {width,height}=cpu.meta,x0=Math.floor(width/4),y0=Math.floor(height/4),x1=Math.max(x0+1,Math.ceil(width*3/4)),y1=Math.max(y0+1,Math.ceil(height*3/4));
          const tileOptions={...options,region:[x0,y0,x1,y1]},tile=await engine.render(5,tileOptions),tileCpu=await engine.render(2,tileOptions);
          if(tile.meta.fallback)throw new Error('A smaller tile lost geometry admission');
          const tileJob=gpu.prepare(tile.data);try{const out=await tileJob.render();entry.tile_accuracy=pixelDifference(out.pixels,tileCpu.data);}finally{tileJob.destroy();}
          variants+=2;
        }
        cases.push(entry);admitted++;
      }
      finally{job.destroy();}
    } catch(error){failures.push({id:fixture.id,error:String(error)});}
    if((admitted+fallback)%100===0)console.log(`GPU coverage fixtures ${admitted+fallback}/${fixtures.length}`);
  }
  return {adapter:gpu.info,fixtures:fixtures.length,admitted,fallback,variants,oracle,precision,cases,failures};
}
function refsAtSafe(words){return words[12];}
