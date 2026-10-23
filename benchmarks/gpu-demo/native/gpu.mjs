/** The same retained-job interface as WebGPU, through the Metal test host. */
import {validateRaster} from '../raster.mjs';
export class MetalBackend {
  constructor(host, info) {Object.assign(this,{host,info,next:1,readbackCopies:1});}
  control(op,id,payload) {
    const bytes=Buffer.alloc(8+(payload?.length??0));bytes.writeUInt32LE(op);bytes.writeUInt32LE(id,4);if(payload)bytes.set(payload,8);
    return this.host.request(bytes);
  }
  async prepare(bytes) {
    const packetSize=bytes.length;const geometry=validateRaster(bytes);delete geometry.words;
    const id=this.next++,setup=await this.control(1,id,bytes);let destroyed=false,dispatches=0;
    return {width:geometry.width,height:geometry.height,geometry,spans:geometry.marks,references:geometry.references,
      setup_ms:setup.meta.setup_ms,packet_bytes:packetSize,resident_bytes:setup.meta.resident_bytes,
      retain:async()=>({upload_ms:setup.meta.upload_ms}),
      counters:()=>({uploads:1,upload_bytes:packetSize,dispatches,resident:!destroyed}),
      render:async upload=>{
        if(destroyed||upload!==null)throw new Error('Native retained job requires immutable resident geometry');
        const output=await this.control(2,id);dispatches=output.meta.dispatches;
        return {pixels:output.data,wall_ms:output.meta.complete_ms,kernel_ms:output.meta.kernel_ms,host_ms:output.host_ms};
      },
      destroy:async()=>{if(!destroyed){await this.control(3,id);destroyed=true;}},
    };
  }
}
