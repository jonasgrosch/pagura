/** The same retained/display interfaces as WebGPU, through the Metal host. */
import {validateRaster} from '../raster.mjs';
import {validateWorld,worldView} from '../world.mjs';
import {viewport} from '../present.mjs';
const words=(...values)=>{const out=Buffer.alloc(values.length*4);values.forEach((v,i)=>out.writeUInt32LE(v,i*4));return out;};
export class MetalBackend {
  constructor(host, info) {Object.assign(this,{host,info,next:1,readbackCopies:1});}
  control(op,id,payload) {
    const bytes=Buffer.alloc(8+(payload?.length??0));bytes.writeUInt32LE(op);bytes.writeUInt32LE(id,4);if(payload)bytes.set(payload,8);
    return this.host.request(bytes);
  }
  async prepare(bytes) {
    const packetSize=bytes.length,geometry=validateRaster(bytes);delete geometry.words;
    const id=this.next++,setup=await this.control(1,id,bytes);let destroyed=false,dispatches=0,rgbaReadbacks=0;
    return {width:geometry.width,height:geometry.height,geometry,spans:geometry.marks,references:geometry.references,
      setup_ms:setup.meta.setup_ms,packet_bytes:packetSize,resident_bytes:setup.meta.resident_bytes,
      retain:async()=>({upload_ms:setup.meta.upload_ms}),
      counters:()=>({uploads:1,upload_bytes:packetSize,dispatches,rgba_readbacks:rgbaReadbacks,resident:!destroyed}),
      render:async(upload,{readback=true,presenter=null}={})=>{
        if(destroyed||upload!==null)throw new Error('Native retained job requires immutable resident geometry');
        let output;
        if(presenter){const capture=presenter.captureNext;presenter.captureNext=false;
          output=await this.control(5,id,words((readback?1:0)|2|(capture?4:0),presenter.position==='center'?1:0));
          if(capture)presenter.capturePending=output.data;
        }else output=readback?await this.control(2,id):await this.control(5,id,words(0,0));
        dispatches=output.meta.dispatches;rgbaReadbacks=output.meta.rgba_readbacks;
        return {pixels:readback?output.data:undefined,wall_ms:output.meta.complete_ms,kernel_ms:output.meta.kernel_ms,host_ms:output.host_ms,presentation:output.meta};
      },
      destroy:async()=>{if(!destroyed){await this.control(3,id);destroyed=true;}},
    };
  }
  async prepareWorld(bytes,meta) {
    const validated=validateWorld(bytes),world={...validated,words:validated.words.slice(0,validated.words[9])};world.floats=new Float32Array(world.words.buffer);
    const id=this.next++,setup=await this.control(10,id,bytes);let state={geometry_uploads:1,geometry_upload_bytes:bytes.length,dispatches:0,rgba_readbacks:0,cached_dpis:[]},destroyed=false;const sizes=new Map();
    return {
      setup_ms:setup.meta.setup_ms,hasView:dpi=>state.cached_dpis.includes(dpi),metadata:()=>({...state,cpu_metadata_bytes:world.words.byteLength}),
      draw:async(dpi,{readback=false,presenter=null,patches=null}={})=>{
        if(destroyed)throw new Error('Native world job destroyed');const start=performance.now();let cache_hit=state.cached_dpis.includes(dpi);
        if(!cache_hit){const view=worldView(world,meta,dpi,patches);state=(await this.control(11,id,Buffer.concat([words(dpi),view.bytes]))).meta;sizes.set(dpi,[view.width,view.height]);for(const key of sizes.keys())if(!state.cached_dpis.includes(key))sizes.delete(key);}
        const capture=presenter?.captureNext??false;if(presenter)presenter.captureNext=false;
        const output=await this.control(12,id,words(dpi,(readback?1:0)|(presenter?2:0)|(capture?4:0),presenter?.position==='center'?1:0));state=output.meta;
        if(capture)presenter.capturePending=output.data;
        const [width,height]=sizes.get(dpi);
        return {pixels:readback?output.data:undefined,width,height,wall_ms:performance.now()-start,device_ms:output.meta.complete_ms,kernel_ms:output.meta.kernel_ms,view_cache_hit:cache_hit,presentation:output.meta};
      },
      destroy:async()=>{if(!destroyed){await this.control(3,id);destroyed=true;}},
    };
  }
}
export class NativePresentation {
  constructor(gpu){Object.assign(this,{gpu,position:'top',captureNext:false,capturePending:null});}
  async cpu(pixels,width,height,position='top'){
    const bytes=viewport(pixels,width,height,480,position),out=await this.gpu.control(30,0,Buffer.concat([words(this.captureNext?4:0),bytes]));
    if(this.captureNext)this.capturePending=out.data;this.captureNext=false;return out.meta;
  }
  async capture(){if(!this.capturePending)throw new Error('No native frame capture');const pixels=this.capturePending;this.capturePending=null;return pixels;}
  destroy(){}
}
