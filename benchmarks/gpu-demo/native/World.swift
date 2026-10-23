// Immutable vector buffers and bounded, independently validated view patches.
import Foundation
import Metal
func validateWorld(_ data:Data) throws -> [UInt32] {
    guard data.count>=96,data.count<=64*1024*1024,data.count%4==0 else {throw fail("Native world packet size")}
    let w=data.withUnsafeBytes {Array($0.bindMemory(to:UInt32.self))}
    guard w[0]==0x34555047,w[1]==1,w[2]>0,w[3]>0,UInt64(w[2])*UInt64(w[3])<=16*1024*1024,
        w[4]==(w[2]+31)/32,w[5]==w[4]*((w[3]+7)/8),w[6]==72,w[7]==300,w[18]>>24==255,w[19]==8 else {throw fail("Native world header")}
    let marks=Int(w[13]),edges=Int(w[14]),rows=Int(w[15]),bins=Int(w[5]),refs=Int(w[16])
    guard w[8]==24,Int(w[9])==24+marks*24,Int(w[10])==Int(w[9])+edges*8,
        Int(w[20])==Int(w[10])+rows*2,Int(w[11])==Int(w[20])+Int(w[21]),Int(w[12])==Int(w[11])+bins*2,
        Int(w[12])+refs==w.count,w[17]==w.count else {throw fail("Native world sections")}
    let f:(Int)->Double={Double(Float(bitPattern:w[$0]))}
    for i in 0..<edges {let at=Int(w[9])+i*8;guard w[at]<=1,w[at+7]==0,(1...6).allSatisfy({f(at+$0).isFinite && abs(f(at+$0))<=16_777_216}) else {throw fail("Native world edge")}}
    var maxRows=[Int](),previous:UInt32=0
    for i in 0..<rows {let at=Int(w[10])+i*2,a=w[at],b=w[at+1];guard a==previous,a<=b,b<=w[21] else {throw fail("Native world row")};previous=b;maxRows.append(Int(b-a))}
    guard previous==w[21] else {throw fail("Native world rows incomplete")}
    for i in 0..<Int(w[21]) {guard w[Int(w[20])+i]<edges else {throw fail("Native world edge reference")}}
    var identities=Set<UInt32>()
    for i in 0..<marks {
        let at=24+i*24
        guard w[at]<w[at+2],w[at+1]<w[at+3],w[at+2]<=w[2],w[at+3]<=w[3],w[at+6]<=1,w[at+7]<=1,w[at+11]>>16==65535,
            w[at+4]<=rows,w[at+5]>0,UInt64(w[at+4])+UInt64(w[at+5])<=rows,w[at+14]<w[22],w[at+15]==0,w[at+19]<=1,w[at+8]<=w[2],w[at+9]<=w[3],
            [12,16,17,20,21,22,23].allSatisfy({f(at+$0).isFinite && abs(f(at+$0))<=16_777_216}),identities.insert(w[at+18]).inserted else {throw fail("Native world mark")}
        if w[at+6] != 0 {guard f(at+20)<f(at+22),f(at+21)<f(at+23) else {throw fail("Native world rectangle clip")}}
    }
    previous=0;var work:UInt64=0
    for bin in 0..<bins {
        let at=Int(w[11])+bin*2,a=w[at],b=w[at+1];guard a==previous,a<=b,b<=refs else {throw fail("Native world bin")};previous=b;var last:Int = -1
        for j in Int(a)..<Int(b) {
            let n=Int(w[Int(w[12])+j]);guard n<marks,n>last else {throw fail("Native world source order")};last=n
            let m=24+n*24,y=Double(bin/Int(w[4])*8)-Double(w[m+9])-f(m+17),top=f(m+12)
            let low=Int(min(Double(w[m+5]),max(0,floor((y-top)/8)-1))),high=Int(min(Double(w[m+5]),max(0,ceil((y+8-top)/8)+1)))
            var weight=0;if low<high {for r in low..<high {weight=max(weight,maxRows[Int(w[m+4])+r])}}
            work+=UInt64(weight+1)*256;guard work<=2_000_000_000 else {throw fail("Native world shader work budget")}
        }
    }
    guard previous==refs else {throw fail("Native world bins incomplete")};return w
}
@MainActor final class WorldJob {
    struct View {let parameters:MTLBuffer,width:Int,height:Int;var bytes:Int {parameters.length}}
    let source:MTLBuffer,output:MTLBuffer,marks:Int,setupMs:Double,device:MTLDevice
    private var views:[UInt32:View]=[:],order=[UInt32]()
    var dispatches=0,viewUploads=0,viewUploadBytes=0,rgbaReadbacks=0,evictions=0
    var bytes:Int {source.length+output.length+views.values.reduce(0,{$0+$1.bytes})}
    init(_ data:Data,device:MTLDevice) throws {
        let started=now(),w=try validateWorld(data);self.device=device;marks=Int(w[13])
        guard let source=device.makeBuffer(length:data.count,options:.storageModeShared),let output=device.makeBuffer(length:Int(w[2])*Int(w[3])*4,options:.storageModeShared) else {throw fail("Native world allocation")}
        self.source=source;self.output=output;data.copyBytes(to:source.contents().assumingMemoryBound(to:UInt8.self),count:data.count);setupMs=now()-started
    }
    func install(_ dpi:UInt32,data:Data,otherBytes:Int) throws {
        guard (72...300).contains(dpi),data.count>=32,data.count%4==0,data.count<=64*1024*1024+marks*48 else {throw fail("Native world view size")}
        let w=data.withUnsafeBytes {Array($0.bindMemory(to:UInt32.self))}
        guard w[0]>0,w[1]>0,UInt64(w[0])*UInt64(w[1])*4<=output.length,w[3]==dpi,w[4]==8+marks*12,w[4]<=w.count,(w.count-Int(w[4]))%4==0,
            abs(Double(Float(bitPattern:w[2]))-Double(dpi)/300)<1e-6 else {throw fail("Native world view header")}
        let cells=(w.count-Int(w[4]))/4
        for i in 0..<marks {
            let at=8+i*12
            guard w[at]<=w[at+2],w[at+1]<=w[at+3],w[at+2]<=w[0],w[at+3]<=w[1],w[at+8]<=w[at+9],w[at+9]<=cells,
                Float(bitPattern:w[at+6]).isFinite,Float(bitPattern:w[at+7]).isFinite else {throw fail("Native world view mark")}
        }
        for i in 0..<cells {let at=Int(w[4])+i*4;guard w[at]<w[1],w[at+1]<w[0],w[at+2]==w[at+1]+1,w[at+3]<=255 else {throw fail("Native world patch")}}
        guard views[dpi]==nil else {throw fail("Native world view is immutable")}
        let required=data.count
        guard required+source.length+output.length+otherBytes<=256*1024*1024 else {throw fail("Native world view budget")}
        while views.count>=3 || bytes+required+otherBytes>256*1024*1024 {let old=order.removeFirst();views.removeValue(forKey:old);evictions+=1}
        guard let params=device.makeBuffer(length:data.count,options:.storageModeShared) else {throw fail("Native world view allocation")}
        data.copyBytes(to:params.contents().assumingMemoryBound(to:UInt8.self),count:data.count)
        views[dpi]=View(parameters:params,width:Int(w[0]),height:Int(w[1]));order.append(dpi);viewUploads+=1;viewUploadBytes+=data.count
    }
    func metadata()->[String:Any] {return ["geometry_uploads":1,"geometry_upload_bytes":source.length,"view_uploads":viewUploads,"view_upload_bytes":viewUploadBytes,"dispatches":dispatches,"rgba_readbacks":rgbaReadbacks,"entries":views.count,"resident_bytes":bytes,"evictions":evictions,"cached_dpis":order]}
    func draw(_ dpi:UInt32,queue:MTLCommandQueue,pipeline:MTLComputePipelineState,readback:Bool,presentation:Presentation?,center:Bool,capture:Bool) throws -> ([String:Any],Data) {
        guard let view=views[dpi],let command=queue.makeCommandBuffer(),let encoder=command.makeComputeCommandEncoder() else {throw fail("Native world view unavailable")}
        let started=now();order.removeAll(where:{$0==dpi});order.append(dpi)
        encoder.setComputePipelineState(pipeline);encoder.setBuffer(source,offset:0,index:0);encoder.setBuffer(output,offset:0,index:1);encoder.setBuffer(view.parameters,offset:0,index:2)
        encoder.dispatchThreads(MTLSize(width:view.width,height:view.height,depth:1),threadsPerThreadgroup:MTLSize(width:64,height:1,depth:1));encoder.endEncoding()
        let frame=try presentation?.encode(command,buffer:output,width:view.width,height:view.height,center:center,capture:capture)
        command.commit();command.waitUntilCompleted();if let error=command.error {throw error};dispatches+=1
        var pixels=Data();if readback {pixels=Data(bytes:output.contents(),count:view.width*view.height*4);rgbaReadbacks+=1}
        var meta=metadata();meta["complete_ms"]=now()-started;meta["kernel_ms"]=(command.gpuEndTime-command.gpuStartTime)*1000
        if let frame,let presentation {let (pm,pc)=try presentation.finish(frame,started:started);meta.merge(pm,uniquingKeysWith:{$1});if capture {pixels=pc}}
        return(meta,pixels)
    }
}
