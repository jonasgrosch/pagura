/** Bounded sequential framed transport; no quadratic buffer concatenation. */
import {spawn} from 'node:child_process';
export class Host {
  constructor(binary, args = [], hello = false) {
    this.chunks = [];this.length = 0;this.pending = null;
    this.ready = hello ? new Promise((resolve,reject) => {this.pending={resolve,reject};}) : Promise.resolve();
    this.child=spawn(binary,args,{stdio:['pipe','pipe','inherit']});
    this.child.stdout.on('data',b=>{this.chunks.push(b);this.length+=b.length;this.receive();});
    this.child.on('error',e=>this.pending?.reject(e));
    this.child.on('exit',c=>this.pending?.reject(new Error(`Native host exit ${c}`)));
  }
  receive(){
    if(this.length<4)return;const header=Buffer.alloc(4);let offset=0;for(const chunk of this.chunks){const count=Math.min(4-offset,chunk.length);chunk.copy(header,offset,0,count);offset+=count;if(offset===4)break;}const n=header.readUInt32LE();
    if(n>68*1024*1024){this.pending?.reject(new Error('Native frame budget'));this.close();return;}
    if(this.length<n+4)return;const data=Buffer.concat(this.chunks);const pending=this.pending;this.pending=null;this.chunks=[];this.length=0;
    try{if(!pending||data.length!==n+4)throw new Error('Unexpected native frame');const size=data.readUInt32LE(4);if(size>n-4)throw new Error('Native metadata budget');const meta=JSON.parse(data.subarray(8,8+size));if(!meta.ok)throw new Error(meta.error);pending.resolve({meta,data:data.subarray(8+size),host_ms:performance.now()-pending.start});}catch(e){pending?.reject(e);}
  }
  request(bytes){return new Promise((resolve,reject)=>{if(this.pending){reject(new Error('Concurrent native requests'));return;}this.pending={resolve,reject,start:performance.now()};const h=Buffer.alloc(4);h.writeUInt32LE(bytes.length);this.child.stdin.write(h);this.child.stdin.write(bytes);});}
  open(pdf){this.generation=(this.generation??0)+1;const b=Buffer.alloc(pdf.length+4);b.writeUInt32LE(1);b.set(pdf,4);return this.request(b);}
  render(op,options=150){this.renderRequests=(this.renderRequests??0)+1;if(typeof options==='number')options={dpi:options};const b=Buffer.alloc(32),values=[op,options.page??0,options.dpi??150,options.band??32,...(options.region??[0,0,0,0])];values.forEach((v,i)=>b.writeUInt32LE(v,i*4));return this.request(b);}
  close(){this.child.stdin.end();this.child.kill('SIGTERM');}
}
