/** GPU buffer -> canvas attachment. No RGBA CPU transfer on display frames. */
export class Presenter {
  static async create(gpu,canvas,size=480) {
    const device=gpu.device,format=navigator.gpu.getPreferredCanvasFormat();
    if(!gpu.presentationPipeline){
      const module=device.createShaderModule({code:await(await fetch('/present.wgsl')).text()});
      gpu.presentationPipeline=await device.createRenderPipelineAsync({layout:'auto',vertex:{module,entryPoint:'triangle'},fragment:{module,entryPoint:'display',targets:[{format}]},primitive:{topology:'triangle-list'}});
    }
    const context=canvas.getContext('webgpu');if(!context)throw new Error('WebGPU canvas unavailable');
    canvas.width=size;canvas.height=size;
    context.configure({device,format,alphaMode:'opaque',colorSpace:'srgb',usage:GPUTextureUsage.RENDER_ATTACHMENT|GPUTextureUsage.COPY_SRC});
    return new Presenter(gpu,canvas,context,format);
  }
  constructor(gpu,canvas,context,format){
    Object.assign(this,{gpu,canvas,context,format,position:'top',presentations:0,captures:0});
    this.uniform=gpu.device.createBuffer({size:16,usage:GPUBufferUsage.UNIFORM|GPUBufferUsage.COPY_DST});this.groups=new WeakMap();
  }
  encode(encoder,buffer,width,height) {
    const {x,y}=viewOrigin(width,height,this.canvas.width,this.position);
    this.gpu.device.queue.writeBuffer(this.uniform,0,new Uint32Array([width,height,x,y]));
    let group=this.groups.get(buffer);
    if(!group){group=this.gpu.device.createBindGroup({layout:this.gpu.presentationPipeline.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer}},{binding:1,resource:{buffer:this.uniform}}]});this.groups.set(buffer,group);}
    const texture=this.context.getCurrentTexture();
    const pass=encoder.beginRenderPass({colorAttachments:[{view:texture.createView(),loadOp:'clear',storeOp:'store',clearValue:{r:1,g:1,b:1,a:1}}]});
    pass.setPipeline(this.gpu.presentationPipeline);pass.setBindGroup(0,group);pass.draw(3);pass.end();this.presentations++;
    if(this.captureNext){
      const row=Math.ceil(this.canvas.width*4/256)*256;
      const staging=this.gpu.device.createBuffer({size:row*this.canvas.height,usage:GPUBufferUsage.COPY_DST|GPUBufferUsage.MAP_READ});
      encoder.copyTextureToBuffer({texture},{buffer:staging,bytesPerRow:row},{width:this.canvas.width,height:this.canvas.height});
      this.capturePending={staging,row};this.captureNext=false;this.captures++;
    }
  }
  async capture() {
    const {staging,row}=this.capturePending??{};if(!staging)throw new Error('No presented frame capture');
    try {
      await staging.mapAsync(GPUMapMode.READ);const mapped=new Uint8Array(staging.getMappedRange()),pixels=new Uint8Array(this.canvas.width*this.canvas.height*4);
      for(let y=0;y<this.canvas.height;y++)pixels.set(mapped.subarray(y*row,y*row+this.canvas.width*4),y*this.canvas.width*4);
      if(this.format.startsWith('bgra'))for(let i=0;i<pixels.length;i+=4){const r=pixels[i];pixels[i]=pixels[i+2];pixels[i+2]=r;}
      staging.unmap();return pixels;
    }finally{staging.destroy();this.capturePending=null;}
  }
  destroy(){this.capturePending?.staging.destroy();this.uniform.destroy();this.context.unconfigure();this.groups=new WeakMap();}
}
export function viewport(pixels,width,height,size=480,position='top') {
  const {x,y}=viewOrigin(width,height,size,position);
  const out=new Uint8Array(size*size*4);out.fill(255);
  for(let row=0;row<Math.min(size,height-y);row++)out.set(pixels.subarray(((y+row)*width+x)*4,((y+row)*width+Math.min(width,x+size))*4),row*size*4);
  return out;
}
export function viewOrigin(width,height,size,position) {
  const x=typeof position==='object'?position.x:position==='center'?Math.max(0,Math.floor((width-size)/2)):0;
  const y=typeof position==='object'?position.y:position==='center'?Math.max(0,Math.floor((height-size)/2)):0;
  if(!Number.isInteger(x)||!Number.isInteger(y)||x<0||y<0||x>=width||y>=height)throw new Error('Presentation offset');
  return {x,y};
}
