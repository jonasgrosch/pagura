// Isolated Metal executor: bounded immutable resources and explicit handles.
import Foundation
import Metal

func now() -> Double { ProcessInfo.processInfo.systemUptime * 1000 }
func fail(_ message: String) -> NSError { NSError(domain: message, code: 1) }
func readExact(_ n: Int) throws -> Data {
    var data = Data()
    while data.count < n {
        guard let part = try FileHandle.standardInput.read(upToCount: n - data.count), !part.isEmpty else { throw fail("EOF") }
        data.append(part)
    }
    return data
}
func reply(_ meta: [String: Any], _ pixels: Data = Data()) throws {
    let json = try JSONSerialization.data(withJSONObject: meta, options: [.sortedKeys])
    var header = Data()
    var length = UInt32(4 + json.count + pixels.count).littleEndian
    var metaLength = UInt32(json.count).littleEndian
    withUnsafeBytes(of: &length) { header.append(contentsOf: $0) }
    withUnsafeBytes(of: &metaLength) { header.append(contentsOf: $0) }
    try FileHandle.standardOutput.write(contentsOf: header)
    try FileHandle.standardOutput.write(contentsOf: json)
    if !pixels.isEmpty { try FileHandle.standardOutput.write(contentsOf: pixels) }

}
func validate(_ data: Data) throws -> [UInt32] {
    guard data.count >= 96, data.count <= 64 * 1024 * 1024, data.count % 4 == 0 else { throw fail("Native packet size") }
    let words = data.withUnsafeBytes { Array($0.bindMemory(to: UInt32.self)) }
        guard words[0] == 0x32555047, (words[1] == 2 || words[1] == 3), words[17] == data.count/4, words[2] > 0, words[3] > 0,
            UInt64(words[2]) * UInt64(words[3]) <= 16 * 1024 * 1024 else { throw NSError(domain: "Invalid native geometry header", code: 1) }
        let shared = words[1] == 3, stride = words[1] == 3 ? 20 : 16
        let marks = Int(words[13]), edges = Int(words[14]), extras = Int(words[15]), bins = Int(words[5]), refs = Int(words[16])
        guard words[8] == 24, Int(words[9]) == 24 + marks * stride,
            Int(words[10]) == Int(words[9]) + edges * 8, Int(words[11]) == Int(words[10]) + extras * 4 + (shared ? Int(words[23]) : 0),
            Int(words[12]) == Int(words[11]) + bins * 2, Int(words[12]) + refs == words.count,
            words[4] == (words[2] + 31) / 32, bins == Int(words[4] * ((words[3] + 7) / 8)) else { throw NSError(domain: "Native geometry sections", code: 1) }
        for i in 0..<marks {
            let at = 24 + i * stride
            guard words[at+4] <= words[at+5], words[at+5] <= (shared ? Int(words[23]) : edges),
                words[at+12] <= words[at+13], words[at+13] <= extras,
                words[at+14] <= words[at+15], words[at+15] <= extras else { throw NSError(domain: "Native mark range", code: 1) }
        }
        if shared {
            guard words[22] == words[10] + UInt32(extras * 4) else { throw fail("Native shared index") }
            for i in 0..<Int(words[23]) { guard words[Int(words[22])+i] < edges else { throw fail("Native shared edge") } }
        }
        var previous: UInt32 = 0, work: UInt64 = 0
        for i in 0..<bins {
            let at = Int(words[11]) + i * 2, a = words[at], b = words[at+1]
            guard a == previous, b >= a, b <= refs else { throw NSError(domain: "Native bin range", code: 1) }
            previous = b
            for j in Int(a)..<Int(b) {
                let mark = Int(words[Int(words[12]) + j])
                guard mark < marks else { throw NSError(domain: "Native reference", code: 1) }
                work += UInt64(words[24+mark*stride+5] - words[24+mark*stride+4] + 1) * 256
            }
        }
        guard previous == refs, work <= 2_000_000_000 else { throw NSError(domain: "Native work budget", code: 1) }
    return words
}

@MainActor final class Job {
    let source: MTLBuffer, output: MTLBuffer, width: Int, height: Int
    let setupMs: Double, uploadMs: Double, allocationMs: Double
    var dispatches = 0, rgbaReadbacks = 0
    var bytes: Int { source.length + output.length }
    init(_ data: Data, words: [UInt32], device: MTLDevice, previous: Job? = nil, started: Double) throws {
        width = Int(words[2]);height = Int(words[3])
        let allocation = now(), size = width * height * 4
        if let old = previous, old.source.length == data.count, old.output.length == size {
            source = old.source;output = old.output
        } else {
            guard let input = device.makeBuffer(length: data.count, options: .storageModeShared),
                let result = device.makeBuffer(length: size, options: .storageModeShared) else { throw fail("Metal allocation") }
            source = input;output = result
        }
        allocationMs = now() - allocation
        let upload = now()
        data.copyBytes(to: source.contents().assumingMemoryBound(to: UInt8.self), count: data.count)
        uploadMs = now() - upload
        setupMs = now() - started - uploadMs
    }
    func draw(_ queue: MTLCommandQueue, _ pipeline: MTLComputePipelineState,
        readback: Bool = true, presentation: Presentation? = nil, center: Bool = false, capture: Bool = false) throws -> ([String:Any], Data) {
        let started = now()
        guard let command = queue.makeCommandBuffer(), let encoder = command.makeComputeCommandEncoder() else { throw fail("Metal command") }
        encoder.setComputePipelineState(pipeline);encoder.setBuffer(source, offset: 0, index: 0);encoder.setBuffer(output, offset: 0, index: 1)
        encoder.dispatchThreads(MTLSize(width: width, height: height, depth: 1), threadsPerThreadgroup: MTLSize(width: 64, height: 1, depth: 1))
        encoder.endEncoding()
        let frame = try presentation?.encode(command,buffer:output,width:width,height:height,center:center,capture:capture)
        command.commit();command.waitUntilCompleted();if let error = command.error { throw error }
        var pixels = Data()
        if readback {pixels = Data(bytes: output.contents(), count: output.length);rgbaReadbacks += 1}
        dispatches += 1
        var meta:[String:Any] = ["ok":true,"complete_ms":now()-started,"kernel_ms":(command.gpuEndTime-command.gpuStartTime)*1000,
            "dispatches":dispatches,"uploads":1,"upload_bytes":source.length,"rgba_readbacks":rgbaReadbacks]
        if let frame,let presentation {let (pm,pc)=try presentation.finish(frame,started:started);meta.merge(pm,uniquingKeysWith:{$1});if capture {pixels=pc}}
        return (meta,pixels)
    }

}

@MainActor func runHost() throws {
    let start = now()
    guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else { throw fail("No Metal GPU") }
    let options = MTLCompileOptions();options.mathMode = .safe
    var fp64 = false, fp64Reason = ""
    do {
        _ = try device.makeLibrary(source:"#include <metal_stdlib>\nusing namespace metal; kernel void fp64(device double* a [[buffer(0)]], uint i [[thread_position_in_grid]]) { a[i] = a[i] * 2.0; }",options:options)
        fp64 = true
    } catch { fp64Reason = error.localizedDescription }
    let shaderURL = URL(fileURLWithPath:CommandLine.arguments[1])
    let area = try String(contentsOf:shaderURL.deletingLastPathComponent().appendingPathComponent("area.metal"),encoding:.utf8)
    let folder=shaderURL.deletingLastPathComponent()
    let worldCode=try String(contentsOf:folder.appendingPathComponent("world.metal"),encoding:.utf8)
    let presentCode=try String(contentsOf:folder.appendingPathComponent("present.metal"),encoding:.utf8)
    let library = try device.makeLibrary(source: "#include <metal_stdlib>\nusing namespace metal;\n" + area + String(contentsOf:shaderURL,encoding:.utf8) + worldCode + presentCode,options:options)
    guard let function = library.makeFunction(name:"rasterize") else { throw fail("No raster kernel") }
    let pipeline = try device.makeComputePipelineState(function:function)
    guard let wf=library.makeFunction(name:"worldRasterize") else {throw fail("World kernel missing")}
    let worldPipeline=try device.makeComputePipelineState(function:wf)
    var painter:Presentation?
    func screen() throws -> Presentation {if painter == nil {painter=try Presentation(device,library:library);try painter!.warmup(queue:queue)};return painter!}
    try reply(["ok":true,"device":device.name,"native_fp64":fp64,"fp64_compile_diagnostic":fp64Reason,
        "initialization_ms":now()-start,"fast_math":false,"unified_memory":device.hasUnifiedMemory])
    var jobs: [UInt32:Job] = [:], legacy: Job?
    var worlds:[UInt32:WorldJob]=[:]
    while true {
        let frame: Data
        do { frame = try readExact(4) } catch { return }
        let n = frame.withUnsafeBytes { Int(UInt32(littleEndian:$0.loadUnaligned(as:UInt32.self))) }
        guard n >= 8, n <= 64*1024*1024+8, n % 4 == 0 else { throw fail("Native frame size") }
        let data = try readExact(n), started = now()
        let control = data.withUnsafeBytes { (UInt32(littleEndian:$0.loadUnaligned(as:UInt32.self)),UInt32(littleEndian:$0.loadUnaligned(fromByteOffset:4,as:UInt32.self))) }
        try autoreleasepool {
        do {
            if control.0 == 0x32555047 {
                guard jobs.isEmpty && worlds.isEmpty else { throw fail("Release retained jobs before legacy rendering") }
                let words = try validate(data), job = try Job(data,words:words,device:device,previous:legacy,started:started)
                legacy = job
                var (meta,pixels) = try job.draw(queue,pipeline)
                meta["complete_ms"] = (meta["complete_ms"] as! Double) + job.uploadMs;meta["allocation_ms"] = job.allocationMs
                try reply(meta,pixels)
            } else if control.0 == 1 {
                let packet = data.subdata(in:8..<n), words = try validate(packet)
                let bytes = packet.count + Int(words[2])*Int(words[3])*4
                guard jobs[control.1] == nil, jobs.count < 3,
                    jobs.values.reduce(0,{$0+$1.bytes})+worlds.values.reduce(0,{$0+$1.bytes})+bytes <= 256*1024*1024 else { throw fail("Native retained resource budget") }
                legacy = nil
                let job = try Job(packet,words:words,device:device,started:started);jobs[control.1] = job
                try reply(["ok":true,"setup_ms":job.setupMs,"upload_ms":job.uploadMs,"resident_bytes":job.bytes,"entries":jobs.count])
            } else if control.0 == 2, n == 8 {
                guard let job = jobs[control.1] else { throw fail("Unknown retained handle") }
                let (meta,pixels) = try job.draw(queue,pipeline);try reply(meta,pixels)
            } else if control.0 == 3, n == 8 {
                let removed=jobs.removeValue(forKey:control.1) != nil || worlds.removeValue(forKey:control.1) != nil
                guard removed else { throw fail("Unknown retained handle") }
                try reply(["ok":true,"entries":jobs.count])
            } else if control.0 == 4, n == 8 {
                jobs.removeAll();worlds.removeAll();legacy = nil;try reply(["ok":true,"entries":0])
            } else if control.0 == 5,n==16 {
                guard let job=jobs[control.1] else {throw fail("Unknown retained handle")}
                let flags=data.withUnsafeBytes {$0.loadUnaligned(fromByteOffset:8,as:UInt32.self)}
                let center=data.withUnsafeBytes {$0.loadUnaligned(fromByteOffset:12,as:UInt32.self)} != 0
                let (meta,pixels)=try job.draw(queue,pipeline,readback:flags&1 != 0,presentation:flags&2 != 0 ? try screen():nil,center:center,capture:flags&4 != 0);try reply(meta,pixels)
            } else if control.0 == 10 {
                guard worlds.isEmpty,jobs[control.1]==nil else {throw fail("Native world handle budget")}
                legacy=nil
                let job=try WorldJob(data.subdata(in:8..<n),device:device)
                guard job.bytes+jobs.values.reduce(0,{$0+$1.bytes})<=256*1024*1024 else {throw fail("Native world resource budget")}
                worlds[control.1]=job;try reply(["ok":true,"setup_ms":job.setupMs,"resident_bytes":job.bytes])
            } else if control.0 == 11,n>=12 {
                guard let job=worlds[control.1] else {throw fail("Unknown native world handle")}
                let dpi=data.withUnsafeBytes {$0.loadUnaligned(fromByteOffset:8,as:UInt32.self)}
                try job.install(dpi,data:data.subdata(in:12..<n),otherBytes:jobs.values.reduce(0,{$0+$1.bytes}))
                var meta=job.metadata();meta["ok"]=true;try reply(meta)
            } else if control.0 == 12,n==20 {
                guard let job=worlds[control.1] else {throw fail("Unknown native world handle")}
                let args=data.withUnsafeBytes {Array($0.bindMemory(to:UInt32.self))}
                var (meta,pixels)=try job.draw(args[2],queue:queue,pipeline:worldPipeline,readback:args[3]&1 != 0,presentation:args[3]&2 != 0 ? try screen():nil,center:args[4] != 0,capture:args[3]&4 != 0)
                meta["ok"]=true;try reply(meta,pixels)
            } else if control.0 == 30,n==12+480*480*4 {
                let flags=data.withUnsafeBytes {$0.loadUnaligned(fromByteOffset:8,as:UInt32.self)}
                var (meta,pixels)=try screen().cpu(data.subdata(in:12..<n),queue:queue,center:false,capture:flags&4 != 0);meta["ok"]=true;try reply(meta,pixels)
            } else { throw fail("Unknown native operation") }
        } catch { try reply(["ok":false,"error":"operation \(control.0): \(error)"]) }
        }
    }
}
@main struct NativeMain {
    @MainActor static func main() {do {try runHost()} catch {try? reply(["ok":false,"error":String(describing:error)]);exit(1)}}
}
