// Isolated experimental Metal executor. No FFI or GPU dependency enters Rust.
import Foundation
import Metal

func now() -> Double { ProcessInfo.processInfo.systemUptime * 1000 }
func readExact(_ n: Int) throws -> Data {
    var data = Data()
    while data.count < n {
        guard let part = try FileHandle.standardInput.read(upToCount: n - data.count), !part.isEmpty else {
            throw NSError(domain: "EOF", code: 1)
        }
        data.append(part)
    }
    return data
}
func reply(_ meta: [String: Any], _ pixels: Data = Data()) throws {
    let json = try JSONSerialization.data(withJSONObject: meta, options: [.sortedKeys])
    var body = Data();var metaLength = UInt32(json.count).littleEndian
    withUnsafeBytes(of: &metaLength) { body.append(contentsOf: $0) };body.append(json);body.append(pixels)
    var length = UInt32(body.count).littleEndian
    var frame = Data();withUnsafeBytes(of: &length) { frame.append(contentsOf: $0) };frame.append(body)
    try FileHandle.standardOutput.write(contentsOf: frame)
}
func main() throws {
    let start = now()
    guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else { throw NSError(domain: "No Metal GPU", code: 1) }
    let options = MTLCompileOptions();options.mathMode = .safe
    var fp64 = false, fp64Reason = ""
    do {
        _ = try device.makeLibrary(source: "#include <metal_stdlib>\nusing namespace metal; kernel void fp64(device double* a [[buffer(0)]], uint i [[thread_position_in_grid]]) { a[i] = a[i] * 2.0; }", options: options)
        fp64 = true
    } catch { fp64Reason = error.localizedDescription }
    let source = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
    let library = try device.makeLibrary(source: source, options: options)
    guard let function = library.makeFunction(name: "rasterize") else { throw NSError(domain: "No raster kernel", code: 1) }
    let pipeline = try device.makeComputePipelineState(function: function)
    try reply(["ok": true, "device": device.name, "native_fp64": fp64, "fp64_compile_diagnostic": fp64Reason,
        "initialization_ms": now() - start, "fast_math": false, "unified_memory": device.hasUnifiedMemory])
    var inputBuffer: MTLBuffer?, outputBuffer: MTLBuffer?
    while true {
        let frame: Data
        do { frame = try readExact(4) } catch { return }
        let n = frame.withUnsafeBytes { Int(UInt32(littleEndian: $0.loadUnaligned(as: UInt32.self))) }
        guard n >= 96, n <= 64 * 1024 * 1024, n % 4 == 0 else { throw NSError(domain: "Invalid native geometry frame", code: 1) }
        let data = try readExact(n)
        let words = data.withUnsafeBytes { Array($0.bindMemory(to: UInt32.self)) }
        guard words[0] == 0x32555047, words[1] == 2, words[17] == n/4, words[2] > 0, words[3] > 0,
            UInt64(words[2]) * UInt64(words[3]) <= 16 * 1024 * 1024 else { throw NSError(domain: "Invalid native geometry header", code: 1) }
        // The Node driver validates all sorted runs and edge values. Bound
        // every native shader index independently before dispatch as well.
        let marks = Int(words[13]), edges = Int(words[14]), extras = Int(words[15]), bins = Int(words[5]), refs = Int(words[16])
        guard words[8] == 24, Int(words[9]) == 24 + marks * 16,
            Int(words[10]) == Int(words[9]) + edges * 8, Int(words[11]) == Int(words[10]) + extras * 4,
            Int(words[12]) == Int(words[11]) + bins * 2, Int(words[12]) + refs == words.count,
            words[4] == (words[2] + 31) / 32, bins == Int(words[4] * ((words[3] + 7) / 8)) else { throw NSError(domain: "Native geometry sections", code: 1) }
        for i in 0..<marks {
            let at = 24 + i * 16
            guard words[at+4] <= words[at+5], words[at+5] <= edges,
                words[at+12] <= words[at+13], words[at+13] <= extras,
                words[at+14] <= words[at+15], words[at+15] <= extras else { throw NSError(domain: "Native mark range", code: 1) }
        }
        var previous: UInt32 = 0, work: UInt64 = 0
        for i in 0..<bins {
            let at = Int(words[11]) + i * 2, a = words[at], b = words[at+1]
            guard a == previous, b >= a, b <= refs else { throw NSError(domain: "Native bin range", code: 1) }
            previous = b
            for j in Int(a)..<Int(b) {
                let mark = Int(words[Int(words[12]) + j])
                guard mark < marks else { throw NSError(domain: "Native reference", code: 1) }
                work += UInt64(words[24+mark*16+5] - words[24+mark*16+4] + 1) * 256
            }
        }
        guard previous == refs, work <= 2_000_000_000 else { throw NSError(domain: "Native work budget", code: 1) }
        let size = Int(words[2]) * Int(words[3]) * 4
        let allocation = now()
        if inputBuffer?.length != n { inputBuffer = device.makeBuffer(length: n, options: .storageModeShared) }
        if outputBuffer?.length != size { outputBuffer = device.makeBuffer(length: size, options: .storageModeShared) }
        guard let input = inputBuffer, let output = outputBuffer else { throw NSError(domain: "Metal allocation", code: 1) }
        let allocationMs = now() - allocation
        let started = now()
        data.copyBytes(to: input.contents().assumingMemoryBound(to: UInt8.self), count: n)
        guard let command = queue.makeCommandBuffer(), let encoder = command.makeComputeCommandEncoder() else { throw NSError(domain: "Metal command", code: 1) }
        encoder.setComputePipelineState(pipeline);encoder.setBuffer(input, offset: 0, index: 0);encoder.setBuffer(output, offset: 0, index: 1)
        encoder.dispatchThreads(MTLSize(width: Int(words[2]), height: Int(words[3]), depth: 1), threadsPerThreadgroup: MTLSize(width: 64, height: 1, depth: 1))
        encoder.endEncoding();command.commit();command.waitUntilCompleted()
        if let error = command.error { throw error }
        let pixels = Data(bytes: output.contents(), count: size)
        try reply(["ok": true, "complete_ms": now() - started, "allocation_ms": allocationMs,
            "kernel_ms": (command.gpuEndTime - command.gpuStartTime) * 1000], pixels)
    }
}

do { try main() } catch {
    try? reply(["ok": false, "error": String(describing: error)])
    exit(1)
}
