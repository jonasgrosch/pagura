// A real CAMetalLayer drawable. CPU and GPU use the same viewport/pipeline.
import AppKit
import Metal
import QuartzCore

struct PresentedFrame {
    let drawable: CAMetalDrawable
    let signal: DispatchSemaphore
    let capture: MTLBuffer?
}
@MainActor final class Presentation {
    let window: NSWindow, layer: CAMetalLayer, pipeline: MTLRenderPipelineState, device: MTLDevice
    let size = 480, row = 2048
    var frames = 0, captures = 0, initialDiscarded = 0, droppedFrames = 0, initializationMs = 0.0
    private var warming = false
    private var cpuBuffer: MTLBuffer?
    init(_ device: MTLDevice, library: MTLLibrary) throws {
        self.device = device
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        window = NSWindow(contentRect:NSRect(x:40,y:80,width:480,height:488),styleMask:[.titled,.closable],backing:.buffered,defer:false)
        window.title = "Pagura · native rendering experiment"
        window.isReleasedWhenClosed = false
        layer = CAMetalLayer();layer.device = device;layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = false;layer.contentsScale = 1;layer.drawableSize = CGSize(width:480,height:488)
        layer.colorspace = CGColorSpace(name:CGColorSpace.sRGB)
        layer.displaySyncEnabled = true;layer.maximumDrawableCount = 3
        let view = NSView(frame:NSRect(x:0,y:0,width:480,height:488));view.wantsLayer = true;view.layer = layer
        window.contentView = view;window.orderFrontRegardless();app.finishLaunching();app.updateWindows();CATransaction.flush()
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name:"presentVertex")
        descriptor.fragmentFunction = library.makeFunction(name:"presentFragment")
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        pipeline = try device.makeRenderPipelineState(descriptor:descriptor)
        window.makeKeyAndOrderFront(nil);app.activate();app.updateWindows();CATransaction.flush()
        RunLoop.main.run(until:Date(timeIntervalSinceNow:0.05))
    }
    func encode(_ command: MTLCommandBuffer, buffer: MTLBuffer, width: Int, height: Int, center: Bool, capture: Bool, offset: SIMD2<UInt32>? = nil) throws -> PresentedFrame {
        window.orderFrontRegardless();NSApp.updateWindows();CATransaction.flush()
        guard let drawable = layer.nextDrawable() else {throw fail("No native drawable")}
        let pass = MTLRenderPassDescriptor();pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .clear;pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColorMake(1,1,1,1)
        guard let encoder = command.makeRenderCommandEncoder(descriptor:pass) else {throw fail("Native presentation encoder")}
        let origin = offset ?? SIMD2<UInt32>(center ? UInt32(max(0,(width-size)/2)) : 0,center ? UInt32(max(0,(height-size)/2)) : 0)
        guard origin.x < max(1,width), origin.y < max(1,height), buffer.length >= width*height*4 else {throw fail("Presentation buffer or offset")}
        var view = SIMD4<UInt32>(UInt32(width),UInt32(height),origin.x,origin.y)
        encoder.setRenderPipelineState(pipeline);encoder.setFragmentBuffer(buffer,offset:0,index:0)
        encoder.setFragmentBytes(&view,length:16,index:1);var marker=UInt32(frames+1);encoder.setFragmentBytes(&marker,length:4,index:2);encoder.drawPrimitives(type:.triangle,vertexStart:0,vertexCount:3);encoder.endEncoding()
        var staging: MTLBuffer?
        if capture {
            staging = device.makeBuffer(length:row*size,options:.storageModeShared)
            guard let staging, let blit = command.makeBlitCommandEncoder() else {throw fail("Native presentation capture")}
            blit.copy(from:drawable.texture,sourceSlice:0,sourceLevel:0,sourceOrigin:MTLOrigin(x:0,y:0,z:0),sourceSize:MTLSize(width:size,height:size,depth:1),to:staging,destinationOffset:0,destinationBytesPerRow:row,destinationBytesPerImage:row*size)
            blit.endEncoding();captures += 1
        }
        let signal = DispatchSemaphore(value:0)
        drawable.addPresentedHandler {_ in signal.signal()};command.present(drawable);frames += 1
        return PresentedFrame(drawable:drawable,signal:signal,capture:staging)
    }
    func finish(_ frame: PresentedFrame, started: Double) throws -> ([String:Any],Data) {
        let deadline = now()+2000
        while frame.signal.wait(timeout:.now()) != .success {
            if now()>deadline {throw fail("Native drawable presentation timeout")}
            RunLoop.main.run(until:Date(timeIntervalSinceNow:0.001))
        }
        let presented=frame.drawable.presentedTime>0
        if !presented {
            if warming {initialDiscarded += 1} else {droppedFrames += 1}
        }
        var pixels = Data()
        if let staging = frame.capture {
            pixels.reserveCapacity(size*size*4)
            for y in 0..<size {pixels.append(Data(bytes:staging.contents().advanced(by:y*row),count:size*4))}
            pixels.withUnsafeMutableBytes {(raw:UnsafeMutableRawBufferPointer) in
                for i in stride(from:0,to:raw.count,by:4) {let r=raw[i];raw[i]=raw[i+2];raw[i+2]=r}
            }
        }
        return (["drawable_presented":presented,"initial_discarded_frames":initialDiscarded,"dropped_frames":droppedFrames,"presentation_initialization_ms":initializationMs,"drawable_presented_ms":presented ? frame.drawable.presentedTime*1000-started:0,"presentation_complete_ms":now()-started,"presentations":frames,"captures":captures],pixels)
    }
    func warmup(queue:MTLCommandQueue) throws {
        let started=now();warming=true;defer {warming=false}
        let white=Data(repeating:255,count:size*size*4)
        for _ in 0..<3 {
            let (meta,_)=try cpu(white,queue:queue,center:false,capture:false)
            if meta["drawable_presented"] as? Bool == true {initializationMs=now()-started;return}
            RunLoop.main.run(until:Date(timeIntervalSinceNow:0.05))
        }
        throw fail("Native drawable did not initialize")
    }
    func cpu(_ pixels: Data, queue: MTLCommandQueue, center: Bool, capture: Bool) throws -> ([String:Any],Data) {
        guard pixels.count==size*size*4 else {throw fail("Native CPU viewport size")}
        let started = now()
        if cpuBuffer == nil {cpuBuffer=device.makeBuffer(length:pixels.count,options:.storageModeShared)}
        guard let buffer=cpuBuffer,let command=queue.makeCommandBuffer() else {throw fail("CPU presentation allocation")}
        let upload=now();pixels.copyBytes(to:buffer.contents().assumingMemoryBound(to:UInt8.self),count:pixels.count)
        let uploadMs=now()-upload
        let frame=try encode(command,buffer:buffer,width:size,height:size,center:false,capture:capture)
        command.commit();command.waitUntilCompleted();if let error=command.error {throw error}
        let complete=now()-started
        var (meta,data)=try finish(frame,started:started);meta["complete_ms"]=complete;meta["upload_ms"]=uploadMs;return(meta,data)
    }
    func retained(_ buffer: MTLBuffer, width: Int, height: Int, queue: MTLCommandQueue, offset: SIMD2<UInt32> = .zero, capture: Bool = false) throws -> ([String:Any],Data) {
        let started=now()
        guard let command=queue.makeCommandBuffer() else {throw fail("Retained presentation command")}
        let frame=try encode(command,buffer:buffer,width:width,height:height,center:false,capture:capture,offset:offset)
        command.commit();command.waitUntilCompleted();if let error=command.error {throw error}
        return try finish(frame,started:started)
    }
}
