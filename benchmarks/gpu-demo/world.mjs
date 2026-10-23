/** Canonical geometry is immutable; per-view metadata contains no edges. */
export function validateWorld(bytes) {
  if(!(bytes instanceof Uint8Array)||bytes.length<96||bytes.length%4||bytes.length>64*1024*1024)throw new Error('World packet budget');
  const w=new Uint32Array(bytes.length/4),dv=new DataView(bytes.buffer,bytes.byteOffset,bytes.length);for(let i=0;i<w.length;i++)w[i]=dv.getUint32(i*4,true);
  const [magic,version,width,height,columns,bins,minDpi,basis]=w;
  if(magic!==0x34555047||version!==1||basis!==300||minDpi!==72||!width||!height||width*height>16*1024*1024||(w[18]>>>24)!==255||columns!==Math.ceil(width/32)||bins!==columns*Math.ceil(height/8))throw new Error('World header');
  if(w[8]!==24||w[9]!==24+w[13]*24||w[10]!==w[9]+w[14]*8||w[20]!==w[10]+w[15]*2||w[11]!==w[20]+w[21]||w[12]!==w[11]+bins*2||w[17]!==w.length||w.length!==w[12]+w[16]||w[19]!==8)throw new Error('World sections');
  const f=new Float32Array(w.buffer),maxRows=[];
  for(let i=0;i<w[14];i++){const at=w[9]+i*8;if(w[at]>1||w[at+7]||![1,2,3,4,5,6].every(j=>Number.isFinite(f[at+j])&&Math.abs(f[at+j])<=16_777_216))throw new Error('World edge');}
  let previous=0;
  for(let i=0;i<w[15];i++){const a=w[w[10]+i*2],b=w[w[10]+i*2+1];if(a!==previous||a>b||b>w[21])throw new Error('World row');previous=b;maxRows.push(b-a);}
  if(previous!==w[21])throw new Error('World row coverage');
  for(let i=0;i<w[21];i++)if(w[w[20]+i]>=w[14])throw new Error('World edge reference');
  const serials=new Set();
  for(let i=0;i<w[13];i++){
    const at=24+i*24;
    if(w[at]>=w[at+2]||w[at+1]>=w[at+3]||w[at+2]>width||w[at+3]>height||(w[at+11]>>>16)!==65535||w[at+6]>1||w[at+7]>1||w[at+14]>=w[22]||w[at+15]||w[at+19]>1||w[at+8]>width||w[at+9]>height)throw new Error('World mark');
    if(w[at+4]+w[at+5]>w[15]||!w[at+5]||![12,16,17,20,21,22,23].every(j=>Number.isFinite(f[at+j])&&Math.abs(f[at+j])<=16_777_216))throw new Error('World source/placement');
    if(w[at+6]&&(f[at+20]>=f[at+22]||f[at+21]>=f[at+23]))throw new Error('World rectangle clip');
    if(serials.has(w[at+18]))throw new Error("Duplicate world source identity");serials.add(w[at+18]);
  }
  previous=0;let work=0;
  for(let i=0;i<bins;i++){
    const a=w[w[11]+i*2],b=w[w[11]+i*2+1];if(a!==previous||a>b||b>w[16])throw new Error('World bin');previous=b;let last=-1;
    for(let j=a;j<b;j++){const n=w[w[12]+j];if(n>=w[13]||n<=last)throw new Error('World source order');last=n;
      const mark=24+n*24,y=(Math.floor(i/columns))*8-w[mark+9]-f[mark+17],top=f[mark+12];
      const a=Math.max(0,Math.floor((y-top)/8)-1),b=Math.min(w[mark+5],Math.ceil((y+8-top)/8)+1);let weight=0;
      for(let r=a;r<b;r++)weight=Math.max(weight,maxRows[w[mark+4]+r]);
      work+=(weight+1)*256;}
  }
  if(previous!==w[16]||work>2_000_000_000)throw new Error(`World shader work budget: ${work}`);
  return {words:w,floats:f,width,height,basis,minDpi,marks:w[13],edges:w[14],sources:w[22],omitted_canonical_cells:w[23],work};
}
export function validatePatches(bytes) {
  if(bytes.length<32||bytes.length%4||bytes.length>64*1024*1024)throw new Error('World patch budget');
  const w=new Uint32Array(bytes.length/4),dv=new DataView(bytes.buffer,bytes.byteOffset,bytes.length);for(let i=0;i<w.length;i++)w[i]=dv.getUint32(i*4,true);
  if(w[0]!==0x31505747||w[1]!==1||w[6]!==8||w[7]!==8+w[4]*16||w.length!==w[7]+w[5]*4)throw new Error('World patch sections');
  const entries=new Map();let previous=0;
  for(let i=0;i<w[4];i++){
    const at=8+i*16,start=w[at+12],end=w[at+13];
    if(entries.has(w[at])||w[at+1]>=w[at+3]||w[at+2]>=w[at+4]||w[at+3]>w[2]||w[at+4]>w[3]||w[at+7]>1||w[at+14]||w[at+15]||start!==previous||end<start||end>w[5])throw new Error('World patch mark');
    const buffer=new DataView(w.buffer),x=buffer.getFloat64((at+8)*4,true),y=buffer.getFloat64((at+10)*4,true);
    if(!Number.isFinite(x)||!Number.isFinite(y))throw new Error('World patch placement');
    let lastY=-1,lastX=-1;
    for(let j=start;j<end;j++){const cell=w[7]+j*4,cy=w[cell],cx=w[cell+1];
      if(cy<lastY||cy===lastY&&cx<=lastX||cy>=w[3]||cx>=w[2]||w[cell+2]!==cx+1||w[cell+3]>255)throw new Error('World patch cell');lastY=cy;lastX=cx;}
    entries.set(w[at],{at,x,y});previous=end;
  }
  if(previous!==w[5])throw new Error('Incomplete world patches');
  return {words:w,entries,width:w[2],height:w[3],cells:w[5]};
}
export function worldView(world,meta,dpi,patchBytes=null) {
  if(!Number.isInteger(dpi)||dpi<72||dpi>300)throw new Error('World vectorization supports 72–300 DPI');
  const patches=patchBytes?validatePatches(patchBytes):null;
  const width=patches?.width??Math.ceil(meta.extent_width*dpi/72-1e-9),height=patches?.height??Math.ceil(meta.extent_height*dpi/72-1e-9);
  if(!width||!height||width*height>16*1024*1024)throw new Error('World output budget');
  if(width>world.width||height>world.height)throw new Error('World view exceeds canonical output budget');
  const scale=dpi/world.basis,w=world.words,f=world.floats,patchAt=8+world.marks*12;
  const view=new Uint32Array(patchAt+(patches?.cells??0)*4),vf=new Float32Array(view.buffer);view.set([width,height]);vf[2]=scale;view[3]=dpi;view[4]=patchAt;
  const seen=new Set();
  for(let i=0;i<world.marks;i++){
    const at=24+i*24,v=8+i*12,x=w[at+8]*scale,y=w[at+9]*scale,ix=Math.floor(x),iy=Math.floor(y);
    const p=patches?.entries.get(w[at+18]);
    if(patches&&!p)continue;
    view.set([Math.max(0,Math.floor(w[at]*scale)-2),Math.max(0,Math.floor(w[at+1]*scale)-2),Math.min(width,Math.ceil(w[at+2]*scale)+2),Math.min(height,Math.ceil(w[at+3]*scale)+2),ix,iy],v);
    vf[v+6]=f[at+16]*scale+x-ix;vf[v+7]=f[at+17]*scale+y-iy;
    if(p){
      const pw=patches.words,pa=p.at;seen.add(w[at+18]);
      if(pw[pa+5]!==w[at+10]||pw[pa+6]!==w[at+11])throw new Error('World view paint identity changed');
      view.set(pw.subarray(pa+1,pa+5),v);view[v+8]=pw[pa+12];view[v+9]=pw[pa+13];
      if(w[at+19]&&pw[pa+7]){vf[v+6]=p.x-ix;vf[v+7]=p.y-iy;}
    }
  }
  for(let i=0;i<world.marks;i++){const v=8+i*12;if(!Number.isFinite(vf[v+6])||!Number.isFinite(vf[v+7]))throw new Error('World placement exceeds f32');}
  if(patches){if(seen.size!==patches.entries.size)throw new Error('World source absent from canonical scene');view.set(patches.words.subarray(patches.words[7]),patchAt);}
  return {bytes:new Uint8Array(view.buffer),width,height,dpi,correction_cells:patches?.cells??0};
}
export class WorldSession {
  constructor(engine,gpu,{patches=true}={}){Object.assign(this,{engine,gpu,patches,generation:engine.generation,page:null,job:null,packet:null});}
  async prepare(options) {
    if(options.region?.some(v=>v!==0))return {fallback:'World geometry supports whole-page requests'};
    if(this.generation!==this.engine.generation||this.page!==(options.page??0)){await this.clear();this.generation=this.engine.generation;this.page=options.page??0;}
    if(this.job)return {cache_hit:true,job:this.job};
    if(this.packet?.meta.fallback)return {fallback:this.packet.meta.fallback};
    const packet=await this.engine.render(11,options);this.packet={meta:packet.meta};
    if(packet.meta.fallback)return {fallback:packet.meta.fallback};
    let world;try{world=validateWorld(packet.data);}catch(error){if(String(error).toLowerCase().includes('budget')){this.packet.meta.fallback=String(error);return {fallback:String(error)};}throw error;}
    const start=performance.now();
    this.job=this.gpu.prepareWorld?await this.gpu.prepareWorld(packet.data,packet.meta):await(await import('./world-gpu.mjs')).createWorldJob(this.gpu,packet.data,packet.meta,world);
    this.packet={meta:packet.meta,bytes:packet.data.length,setup_ms:performance.now()-start,geometry:{marks:world.marks,edges:world.edges,sources:world.sources,omitted_canonical_cells:world.omitted_canonical_cells}};
    return {cache_hit:false,job:this.job};
  }
  async draw(options,config={}){
    const dpi=options.dpi??150;if(dpi<72||dpi>300)return {fallback:'World vectorization supports 72–300 DPI'};
    const entry=await this.prepare(options);if(entry.fallback)return entry;
    let patches=null,patchMeta=null;const start=performance.now();
    if(this.patches&&!this.job.hasView(options.dpi??150)){
      const out=await this.engine.render(12,options);if(out.meta.fallback)return {fallback:out.meta.fallback};patches=out.data;patchMeta=out.meta;
    }
    let out;try{out=await this.job.draw(options.dpi??150,{...config,patches});}catch(error){if(String(error).includes('World source absent')||String(error).includes('World placement exceeds'))return {fallback:String(error)};throw error;}
    return {...out,gpu_ms:out.wall_ms,wall_ms:performance.now()-start,patch_meta:patchMeta,geometry_cache_hit:entry.cache_hit};
  }
  async clear(){await this.job?.destroy();this.job=null;this.packet=null;}
}
