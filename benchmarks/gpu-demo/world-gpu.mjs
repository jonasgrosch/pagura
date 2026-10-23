/** WebGPU executor: immutable vectors, one output, bounded view LRU. */
import {worldView} from './world.mjs';
const LIMIT=256*1024*1024;
export async function createWorldJob(gpu,bytes,meta,world) {
  const start=performance.now(),device=gpu.device,packetSize=bytes.length;
  if(!gpu.worldPipeline){
    const code=await(await fetch('/area.wgsl')).text()+await(await fetch('/world.wgsl')).text();
    const module=device.createShaderModule({code});
    const errors=(await module.getCompilationInfo()).messages.filter(m=>m.type==="error");if(errors.length)throw new Error(errors.map(m=>m.message).join("\n"));
    gpu.worldPipeline=await device.createComputePipelineAsync({layout:'auto',compute:{module,entryPoint:'rasterize'}});
  }
  const seed={...world,words:world.words.slice(0,world.words[9])};seed.floats=new Float32Array(seed.words.buffer);
  const source=device.createBuffer({size:packetSize,usage:GPUBufferUsage.STORAGE|GPUBufferUsage.COPY_DST});
  device.queue.writeBuffer(source,0,bytes);await device.queue.onSubmittedWorkDone();
  const queries=gpu.timestamps?device.createQuerySet({type:'timestamp',count:2}):null;
  const resolve=queries?device.createBuffer({size:16,usage:GPUBufferUsage.QUERY_RESOLVE|GPUBufferUsage.COPY_SRC}):null;
  const timing=queries?device.createBuffer({size:16,usage:GPUBufferUsage.MAP_READ|GPUBufferUsage.COPY_DST}):null;
  const outputSize=world.width*world.height*4;
  const output=device.createBuffer({size:outputSize,usage:GPUBufferUsage.STORAGE|GPUBufferUsage.COPY_SRC});
  const slots=new Map();let resident=packetSize+outputSize+(queries?32:0),evictions=0,viewUploads=0,viewBytes=0,dispatches=0,rgbaReadbacks=0,busy=false,destroyed=false;
  const release=slot=>{slot.view.destroy();resident-=slot.bytes;};
  const prepare=(dpi,patches)=>{
    if(slots.has(dpi)){const slot=slots.get(dpi);slots.delete(dpi);slots.set(dpi,slot);return {...slot,cache_hit:true};}
    const started=performance.now(),v=worldView(seed,meta,dpi,patches),size=v.width*v.height*4,required=v.bytes.length;
    if(required+packetSize+outputSize+(queries?32:0)>LIMIT||Math.ceil(v.width/64)>device.limits.maxComputeWorkgroupsPerDimension||v.height>device.limits.maxComputeWorkgroupsPerDimension)throw new Error('World device budget');
    while(slots.size>=3||resident+required>LIMIT){const [key,old]=slots.entries().next().value;release(old);slots.delete(key);evictions++;}
    const view=device.createBuffer({size:v.bytes.length,usage:GPUBufferUsage.STORAGE|GPUBufferUsage.COPY_DST});
    device.queue.writeBuffer(view,0,v.bytes);viewUploads++;viewBytes+=v.bytes.length;
    const group=device.createBindGroup({layout:gpu.worldPipeline.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:source}},{binding:1,resource:{buffer:output}},{binding:2,resource:{buffer:view}}]});
    const slot={width:v.width,height:v.height,dpi,size,output,view,group,bytes:required};slots.set(dpi,slot);resident+=required;
    return {...slot,cache_hit:false,prepare_ms:performance.now()-started};
  };
  return {
    output_buffer:output,
    setup_ms:performance.now()-start,
    metadata:()=>({geometry_uploads:1,geometry_upload_bytes:packetSize,view_uploads:viewUploads,view_upload_bytes:viewBytes,dispatches,rgba_readbacks:rgbaReadbacks,entries:slots.size,resident_bytes:resident,cpu_metadata_bytes:seed.words.byteLength,evictions}),
    hasView:dpi=>slots.has(dpi),
    draw:async(dpi,{readback=false,presenter=null,patches=null}={})=>{
      if(busy||destroyed||gpu.lost())throw new Error(gpu.lost()||'World job unavailable');
      busy=true;const started=performance.now();device.pushErrorScope('validation');let scoped=true,staging=null;
      const timedQueries=readback?queries:null;
      try {
        const slot=prepare(dpi,patches),encoder=device.createCommandEncoder();
        const pass=encoder.beginComputePass(timedQueries?{timestampWrites:{querySet:queries,beginningOfPassWriteIndex:0,endOfPassWriteIndex:1}}:{});
        pass.setPipeline(gpu.worldPipeline);pass.setBindGroup(0,slot.group);pass.dispatchWorkgroups(Math.ceil(slot.width/64),slot.height);pass.end();
        if(presenter)presenter.encode(encoder,slot.output,slot.width,slot.height);
        if(readback){
          // Diagnostic readback is separate from display frames and explicitly counted.
          if(resident+slot.size>LIMIT)throw new Error('World diagnostic readback budget');
          staging=device.createBuffer({size:slot.size,usage:GPUBufferUsage.COPY_DST|GPUBufferUsage.MAP_READ});
          encoder.copyBufferToBuffer(slot.output,0,staging,0,slot.size);rgbaReadbacks++;
        }
        if(timedQueries){encoder.resolveQuerySet(queries,0,2,resolve,0);encoder.copyBufferToBuffer(resolve,0,timing,0,16);}
        device.queue.submit([encoder.finish()]);dispatches++;
        let pixels,kernel_ms=null;
        if(staging){await staging.mapAsync(GPUMapMode.READ);pixels=new Uint8Array(staging.getMappedRange()).slice();staging.unmap();}
        if(timedQueries){await timing.mapAsync(GPUMapMode.READ);const times=new BigUint64Array(timing.getMappedRange());kernel_ms=Number(times[1]-times[0])/1e6;timing.unmap();}
        else await device.queue.onSubmittedWorkDone();
        const wall_ms=performance.now()-started,error=await device.popErrorScope();scoped=false;if(error)throw new Error(error.message);
        return {pixels,wall_ms,kernel_ms,width:slot.width,height:slot.height,view_cache_hit:slot.cache_hit,prepare_ms:slot.prepare_ms??0};
      }catch(error){if(scoped)await device.popErrorScope().catch(()=>{});throw error;}
      finally{staging?.destroy();busy=false;}
    },
    destroy:()=>{if(destroyed)return;destroyed=true;for(const slot of slots.values())release(slot);slots.clear();source.destroy();output.destroy();queries?.destroy();resolve?.destroy();timing?.destroy();},
  };
}
