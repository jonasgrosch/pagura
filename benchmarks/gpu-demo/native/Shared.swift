// Native host capabilities: private shared files and a metadata-only engine channel.
import Foundation
import Metal
import Darwin

final class SharedRegion {
    static let capacity=64*1024*1024
    let path:URL,pointer:UnsafeMutableRawPointer
    private var metalOwnsMapping=false
    init(_ path:URL) throws {
        self.path=path
        let fd=Darwin.open(path.path,O_RDWR|O_CREAT|O_EXCL,0o600)
        guard fd>=0 else {throw fail("Create shared region")};defer {Darwin.close(fd)}
        guard ftruncate(fd,off_t(Self.capacity))==0 else {throw fail("Size shared region")}
        guard let p=mmap(nil,Self.capacity,PROT_READ|PROT_WRITE,MAP_SHARED,fd,0),p != MAP_FAILED else {throw fail("Map shared region")}
        pointer=p
    }
    deinit {if !metalOwnsMapping {munmap(pointer,Self.capacity)}}
    func metalBuffer(_ device:MTLDevice) throws -> MTLBuffer {
        guard !metalOwnsMapping,let buffer=device.makeBuffer(bytesNoCopy:pointer,length:Self.capacity,options:.storageModeShared,deallocator:{pointer,length in munmap(pointer,length)}) else {throw fail("Metal shared frame buffer")}
        metalOwnsMapping=true;return buffer
    }
    func bytes(_ count:Int) throws -> Data {
        guard (0...Self.capacity).contains(count) else {throw fail("Shared byte extent")}
        // This borrow must be consumed before the next request overwrites it.
        return Data(bytesNoCopy:pointer,count:count,deallocator:.none)
    }
}

@MainActor final class EngineBridge {
    let folder:URL,frame:SharedRegion,packet:SharedRegion,frameBuffer:MTLBuffer,process:Process,input:FileHandle,output:FileHandle
    var requests=0,sharedBytes=0,pipePayloadBytes=0,rgbaBytes=0
    init(binary:URL,device:MTLDevice) throws {
        folder=FileManager.default.temporaryDirectory.appendingPathComponent("pagura-shared-\(UUID().uuidString)",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:false,attributes:[.posixPermissions:0o700])
        frame=try SharedRegion(folder.appendingPathComponent("frame"));packet=try SharedRegion(folder.appendingPathComponent("packet"))
        frameBuffer=try frame.metalBuffer(device)
        let stdin=Pipe(),stdout=Pipe();input=stdin.fileHandleForWriting;output=stdout.fileHandleForReading
        process=Process();process.executableURL=binary;process.arguments=["--shared-output",frame.path.path,packet.path.path]
        process.standardInput=stdin;process.standardOutput=stdout;process.standardError=FileHandle.standardError
        try process.run()
    }
    func close() {
        try? input.close();try? output.close()
        if process.isRunning {process.terminate()};process.waitUntilExit()
        // Every draw waits for GPU completion; mappings outlive their Metal buffer.
        try? FileManager.default.removeItem(at:folder)
    }
    func read(_ n:Int) throws -> Data {
        var data=Data();while data.count<n {
            guard let next=try output.read(upToCount:n-data.count),!next.isEmpty else {throw fail("Engine EOF")};data.append(next)
        };return data
    }
    func request(_ data:Data) throws -> ([String:Any],Data) {
        guard data.count<=SharedRegion.capacity else {throw fail("Engine input budget")}
        var count=UInt32(data.count).littleEndian
        try withUnsafeBytes(of:&count) {try input.write(contentsOf:Data($0))};try input.write(contentsOf:data)
        let header=try read(4),length=header.withUnsafeBytes {Int($0.loadUnaligned(as:UInt32.self))}
        guard (4...16384).contains(length) else {throw fail("Metadata-only response budget")}
        let response=try read(length),metaLength=response.withUnsafeBytes {Int($0.loadUnaligned(as:UInt32.self))}
        guard metaLength==length-4 else {throw fail("Bulk bytes on control channel")}
        guard let meta=try JSONSerialization.jsonObject(with:response.subdata(in:4..<length)) as? [String:Any],meta["ok"] as? Bool == true else {throw fail("Engine: \(String(data:response.dropFirst(4),encoding:.utf8) ?? "invalid response")")}
        guard let size=meta["shared_bytes"] as? Int,meta["pipe_payload_bytes"] as? Int == 0,let kind=meta["shared_kind"] as? String,["rgba","packet"].contains(kind) else {throw fail("Missing shared acknowledgement")}
        if kind=="rgba" {
            guard let width=meta["width"] as? Int,let height=meta["height"] as? Int,width>0,height>0,width<=16*1024*1024,height<=16*1024*1024,width*height<=16*1024*1024,size==width*height*4 else {throw fail("Shared RGBA extent")}
        }
        requests+=1;sharedBytes+=size;if kind=="rgba" {rgbaBytes+=size}
        return (meta,try (kind=="rgba" ? frame:packet).bytes(size))
    }
    func open(_ pdf:Data) throws -> [String:Any] {
        guard pdf.count<=SharedRegion.capacity-4 else {throw fail("PDF size budget")}
        var data=wire([1]);data.append(pdf);return try request(data).0
    }
    func render(_ operation:UInt32,_ dpi:UInt32) throws -> ([String:Any],Data) {try request(wire([operation,0,dpi,32,0,0,0,0]))}
    func counters()->[String:Int] { ["requests":requests,"shared_bytes_written":sharedBytes,"rgba_file_write_bytes":rgbaBytes,"rgba_pipe_bytes":pipePayloadBytes,"rgba_swift_upload_bytes":0] }
}
func wire(_ words:[UInt32])->Data {var data=Data();for var word in words {word=word.littleEndian;withUnsafeBytes(of:&word){data.append(contentsOf:$0)}};return data}

// Convert validated per-view metadata; canonical source coordinates stay immutable.
func nativeWorldView(_ world:[UInt32],patch:Data,dpi:UInt32) throws -> Data {
    guard patch.count>=32,patch.count<=SharedRegion.capacity,patch.count%4==0 else {throw fail("Shared patch size")}
    let p=patch.withUnsafeBytes {Array($0.bindMemory(to:UInt32.self))}
    guard p[0]==0x31505747,p[1]==1,p[2]>0,p[3]>0,UInt64(p[2])*UInt64(p[3])<=16*1024*1024,Int(p[4])<=(p.count-8)/16,Int(p[5])<=(p.count-8)/4,p[6]==8,Int(p[7])==8+Int(p[4])*16,Int(p[7])<=p.count,p.count==Int(p[7])+Int(p[5])*4 else {throw fail("Shared patch sections")}
    let marks=Int(world[13]),start=8+marks*12
    var view=[UInt32](repeating:0,count:start+Int(p[5])*4);view[0]=p[2];view[1]=p[3];view[2]=(Float(dpi)/300).bitPattern;view[3]=dpi;view[4]=UInt32(start)
    var entries=[UInt32:Int](),previous:UInt32=0
    for i in 0..<Int(p[4]) {
        let at=8+i*16
        guard entries[p[at]]==nil,p[at+1]<p[at+3],p[at+2]<p[at+4],p[at+3]<=p[2],p[at+4]<=p[3],p[at+12]==previous,p[at+13]>=previous,p[at+13]<=p[5],p[at+7]<=1,p[at+14]==0,p[at+15]==0 else {throw fail("Shared patch identity/range")}
        var lastY:Int = -1,lastX:Int = -1
        for j in Int(previous)..<Int(p[at+13]) {
            let c=Int(p[7])+j*4,y=Int(p[c]),x=Int(p[c+1])
            guard y>lastY || y==lastY && x>lastX,y<Int(p[3]),x<Int(p[2]),p[c+2]==p[c+1]+1,p[c+3]<=255 else {throw fail("Shared patch ordering")};lastY=y;lastX=x
        }
        entries[p[at]]=at;previous=p[at+13]
    }
    guard previous==p[5] else {throw fail("Shared patch cells incomplete")}
    let scale=Double(dpi)/300;var seen=0
    for i in 0..<marks {
        let m=24+i*24,v=8+i*12
        guard let at=entries[world[m+18]] else {continue};seen+=1
        guard p[at+5]==world[m+10],p[at+6]==world[m+11] else {throw fail("Shared world paint changed")}
        let x=Double(world[m+8])*scale,y=Double(world[m+9])*scale,ix=floor(x),iy=floor(y)
        for j in 0..<4 {view[v+j]=p[at+j+1]};view[v+4]=UInt32(ix);view[v+5]=UInt32(iy);view[v+8]=p[at+12];view[v+9]=p[at+13]
        var px=Double(Float(bitPattern:world[m+16]))*scale+x-ix,py=Double(Float(bitPattern:world[m+17]))*scale+y-iy
        if world[m+19] != 0 && p[at+7] != 0 {
            let bitsX=UInt64(p[at+8])|UInt64(p[at+9])<<32,bitsY=UInt64(p[at+10])|UInt64(p[at+11])<<32
            px=Double(bitPattern:bitsX)-ix;py=Double(bitPattern:bitsY)-iy
        }
        guard px.isFinite,py.isFinite,Float(px).isFinite,Float(py).isFinite else {throw fail("Shared world placement")}
        view[v+6]=Float(px).bitPattern;view[v+7]=Float(py).bitPattern
    }
    guard seen==entries.count else {throw fail("Shared source absent from canonical scene")}
    view.replaceSubrange(start..<view.count,with:p[Int(p[7])...]);return wire(view)
}
