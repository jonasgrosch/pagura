// A native application owns both the engine control channel and Metal. Node
// launches the study and stores its report; it never relays frame requests.
import AppKit
import Metal
import CryptoKit

func option(_ key:String,_ fallback:String)->String {let args=CommandLine.arguments;guard let i=args.firstIndex(of:key),i+1<args.count else {return fallback};return args[i+1]}
func metric(_ meta:[String:Any],_ key:String)->Double {(meta[key] as? NSNumber)?.doubleValue ?? 0}
func digest(_ data:Data)->String {SHA256.hash(data:data).map{String(format:"%02x",$0)}.joined()}
func distribution(_ values:[Double])->[String:Double] {
    let sorted=values.sorted(),n=sorted.count
    return ["median":n%2==0 ? (sorted[n/2-1]+sorted[n/2])/2:sorted[n/2],"p95":sorted[Int(ceil(Double(n)*0.95))-1]]
}
func difference(_ a:Data,_ b:Data) throws -> [String:Any] {
    guard a.count==b.count,a.count%4==0 else {throw fail("Diagnostic pixel extent")}
    var maximum=0,changed=0,beyond=0,total:UInt64=0
    a.withUnsafeBytes {(aa:UnsafeRawBufferPointer) in b.withUnsafeBytes {(bb:UnsafeRawBufferPointer) in
        for at in stride(from:0,to:a.count,by:4) {
            var m=0;for j in 0..<4 {let d=abs(Int(aa[at+j])-Int(bb[at+j]));maximum=max(maximum,d);m=max(m,d);total+=UInt64(d)}
            if m>0 {changed+=1};if m>1 {beyond+=1}
        }
    }}
    return ["max_channel_error":maximum,"mean_channel_error":Double(total)/Double(max(1,a.count)),"changed_pixels":changed,"beyond_one_percent":Double(beyond)*400/Double(max(1,a.count))]
}
func cropped(_ data:Data,width:Int,height:Int,offset:SIMD2<UInt32>)->Data {
    var result=Data(repeating:255,count:480*480*4);let x=Int(offset.x),y=Int(offset.y)
    for row in 0..<min(480,height-y) {
        let count=min(480,width-x)*4,start=((y+row)*width+x)*4
        result.replaceSubrange(row*480*4..<row*480*4+count,with:data[start..<start+count])
    };return result
}

@MainActor final class NativeContext {
    let device:MTLDevice,queue:MTLCommandQueue,screen:Presentation,folder:URL,initializationMs:Double
    private var raster:MTLComputePipelineState?,world:MTLComputePipelineState?
    init(folder:URL) throws {
        let started=now();self.folder=folder
        guard let device=MTLCreateSystemDefaultDevice(),let queue=device.makeCommandQueue() else {throw fail("Metal unavailable")};self.device=device;self.queue=queue
        let options=MTLCompileOptions();options.mathMode = .safe
        let code=try String(contentsOf:folder.appendingPathComponent("present.metal"),encoding:.utf8)
        let library=try device.makeLibrary(source:"#include <metal_stdlib>\nusing namespace metal;\n"+code,options:options)
        screen=try Presentation(device,library:library);try screen.warmup(queue:queue);initializationMs=now()-started
    }
    func pipeline(_ mode:String) throws -> MTLComputePipelineState {
        if let cached=mode=="world" ? world:raster {return cached}
        let options=MTLCompileOptions();options.mathMode = .safe
        let files=mode=="world" ? ["area.metal","raster.metal","world.metal"]:["area.metal","raster.metal"]
        let code=try files.map {try String(contentsOf:folder.appendingPathComponent($0),encoding:.utf8)}.joined(separator:"\n")
        let library=try device.makeLibrary(source:"#include <metal_stdlib>\nusing namespace metal;\n"+code,options:options)
        guard let function=library.makeFunction(name:mode=="world" ? "worldRasterize":"rasterize") else {throw fail("Compute entry point")}
        let result=try device.makeComputePipelineState(function:function)
        if mode=="world" {world=result} else {raster=result};return result
    }
}

@MainActor final class NativePage {
    let engine:EngineBridge,context:NativeContext,mode:String
    var fallback:String?,jobs:[UInt32:Job]=[:],order=[UInt32](),world:WorldJob?,worldWords=[UInt32]()
    var width=0,height=0,currentDpi:UInt32=0,lastMeta:[String:Any]=[:]
    var buffer:MTLBuffer {if let world,mode=="world",fallback==nil {return world.output};if mode=="retained",fallback==nil,let job=jobs[currentDpi] {return job.output};return engine.frameBuffer}
    init(_ mode:String,binary:URL,context:NativeContext) throws {self.mode=mode;self.context=context;engine=try EngineBridge(binary:binary,device:context.device)}
    func close() {jobs.removeAll();world=nil;engine.close()}
    func render(_ dpi:UInt32,present:Bool=true) throws -> [String:Any] {
        let started=now();var prepare=0.0,cache=false,draw:[String:Any]=[:]
        if mode=="retained",fallback==nil,jobs[dpi]==nil {
            let (meta,packet)=try engine.render(8,dpi);lastMeta=meta
            if let reason=meta["fallback"] as? String {fallback=reason}
            else {
                let job=try Job(packet,words:validate(packet),device:context.device,started:now())
                guard job.bytes<=256*1024*1024 else {throw fail("Shared retained job budget")}
                while jobs.count>=3 || jobs.values.reduce(0,{$0+$1.bytes})+job.bytes>256*1024*1024 {jobs.removeValue(forKey:order.removeFirst())}
                jobs[dpi]=job;order.append(dpi)
            }
        } else if mode=="world",fallback==nil {
            if world==nil {
                let (meta,packet)=try engine.render(11,dpi);lastMeta=meta
                if let reason=meta["fallback"] as? String {fallback=reason}
                else {worldWords=try validateWorld(packet);world=try WorldJob(packet,device:context.device)}
            }
            if let world,!world.hasView(dpi) {
                let (meta,patch)=try engine.render(12,dpi);lastMeta=meta
                if let reason=meta["fallback"] as? String {fallback=reason}
                else {try world.install(dpi,data:nativeWorldView(worldWords,patch:patch,dpi:dpi),otherBytes:0)}
            } else if world != nil {cache=true}
        } else if mode=="retained",jobs[dpi] != nil {cache=true}
        prepare=now()-started
        if mode=="cpu" || fallback != nil {
            let (meta,_)=try engine.render(7,dpi);lastMeta=meta;width=Int(metric(meta,"width"));height=Int(metric(meta,"height"));cache=meta["cache_hit"] as? Bool ?? false
            prepare=now()-started
            if present {draw=try context.screen.retained(engine.frameBuffer,width:width,height:height,queue:context.queue).0}
        } else if mode=="retained",let job=jobs[dpi] {
            width=job.width;height=job.height;order.removeAll(where:{$0==dpi});order.append(dpi)
            draw=try job.draw(context.queue,context.pipeline(mode),readback:false,presentation:present ? context.screen:nil).0
        } else if let world {
            draw=try world.draw(dpi,queue:context.queue,pipeline:context.pipeline(mode),readback:false,presentation:present ? context.screen:nil,center:false,capture:false).0
            (width,height)=try world.dimensions(dpi)
        } else {throw fail("Shared renderer unavailable")}
        currentDpi=dpi
        let total=now()-started
        // GPU complete_ms excludes drawable waiting; CPU prepare includes safe file writes.
        return ["frame_ms":total,"render_ms":prepare+metric(draw,"complete_ms"),"prepare_ms":prepare,"engine_ms":metric(lastMeta,"engine_ms"),"cache_hit":cache,"fallback":fallback as Any? ?? NSNull(),"presentation":draw]
    }
    func scroll(_ fraction:Double,capture:Bool=false) throws -> ([String:Any],Data,SIMD2<UInt32>) {
        let offset=SIMD2<UInt32>(UInt32(max(0,(width-480)/2)),UInt32(Double(max(0,height-480))*fraction))
        let (meta,pixels)=try context.screen.retained(buffer,width:width,height:height,queue:context.queue,offset:offset,capture:capture)
        return (meta,pixels,offset)
    }
    func accuracy(_ dpi:UInt32) throws -> [String:Any] {
        let (meta,borrowed)=try engine.render(2,dpi),reference=borrowed.withUnsafeBytes {Data(bytes:$0.baseAddress!,count:$0.count)}
        _=try render(dpi,present:false)
        let pixels:Data
        if mode=="cpu" || fallback != nil {pixels=try engine.frame.bytes(width*height*4)}
        else if mode=="retained",let job=jobs[dpi] {pixels=try job.draw(context.queue,context.pipeline(mode),readback:true).1}
        else if let world {pixels=try world.draw(dpi,queue:context.queue,pipeline:context.pipeline(mode),readback:true,presentation:nil,center:false,capture:false).1}
        else {throw fail("No diagnostic pixels")}
        var captures=[[String:Any]]()
        for fraction in [0.0,0.5,1.0] {
            let (_,capture,offset)=try scroll(fraction,capture:true),delta=try difference(capture,cropped(pixels,width:width,height:height,offset:offset))
            guard metric(delta,"max_channel_error")==0 else {throw fail("Shared presentation changed pixels")};captures.append(["fraction":fraction,"difference":delta])
        }
        let delta=try difference(pixels,reference)
        if mode=="cpu" || fallback != nil {guard metric(delta,"max_channel_error")==0 else {throw fail("Shared CPU bands differ")}}
        return ["dpi":dpi,"width":meta["width"]!,"height":meta["height"]!,"cpu_sha256":digest(reference),"pixels_sha256":digest(pixels),"difference":delta,"captures":captures]
    }
    func counters()->[String:Any] {
        ["transport":engine.counters(),"gpu":world?.metadata() ?? ["geometry_uploads":jobs.count,"dispatches":jobs.values.reduce(0,{$0+$1.dispatches}),"rgba_readbacks":jobs.values.reduce(0,{$0+$1.rgbaReadbacks})]]
    }
}

@MainActor func integratedStudy() throws {
    let binary=URL(fileURLWithPath:option("--engine","target/gpu-demo/release/pagura-gpu-demo")),folder=URL(fileURLWithPath:option("--shaders","benchmarks/gpu-demo/native"),isDirectory:true)
    let samples=Int(option("--samples","100")) ?? 0,warmups=Int(option("--warmups","5")) ?? 0,cold=Int(option("--cold-samples","20")) ?? 0
    guard (1...1000).contains(samples),(0...100).contains(warmups),(1...100).contains(cold) else {throw fail("Integrated sample bounds")}
    let context=try NativeContext(folder:folder),args=CommandLine.arguments
    var paths=[String]();for i in 1..<args.count where args[i]=="--pdf" && i+1<args.count {paths.append(args[i+1])}
    let modes=["cpu","retained","world"],dpis:[UInt32]=[72,150,300],novel:[UInt32]=[151,163,177,191,207,221,239,253,269,281,293,299]
    var cases=[[String:Any]]()
    for (index,path) in paths.enumerated() {
        let url=URL(fileURLWithPath:path),read=now(),pdf=try Data(contentsOf:url),fileMs=now()-read
        var opening=[String:[[String:Any]]](),pages=[NativePage]()
        defer {for page in pages {page.close()}}
        for mode in modes {opening[mode]=[]}
        for i in 0..<cold {
            let order=Array(modes[(i%3)...]+modes[..<(i%3)])
            for mode in order {
                let started=now(),page=try NativePage(mode,binary:binary,context:context)
                do {
                    let launch=now()-started,openStart=now(),opened=try page.engine.open(pdf),open=now()-openStart,first=try page.render(150)
                    opening[mode]!.append(["host_launch_ms":launch,"open_request_ms":open,"open_engine_ms":metric(opened,"open_ms"),"first_display_ms":now()-started,"draw":first,"transport":page.engine.counters()])
                } catch {page.close();throw error};page.close()
            }
        }
        for mode in modes {let page=try NativePage(mode,binary:binary,context:context);pages.append(page);_=try page.engine.open(pdf)}
        var staticCases=[[String:Any]](),zoom=[String:[[String:Any]]](),scroll=[String:[[String:Any]]](),accuracy=[String:[[String:Any]]]()
        for dpi in dpis {
            var rows=[String:[[String:Any]]]();for mode in modes {rows[mode]=[]}
            for i in -warmups..<samples {
                var order=Array(0..<3);let shift=(i+warmups)%3;order=Array(order[shift...]+order[..<shift]);if (i+warmups)/3%2==1 {order.reverse()}
                for n in order {let row=try pages[n].render(dpi);if i>=0 {rows[modes[n]]!.append(row)}}
            }
            let summary=rows.mapValues { ["frame_ms":distribution($0.map{metric($0,"frame_ms")}),"render_ms":distribution($0.map{metric($0,"render_ms")})] }
            staticCases.append(["dpi":dpi,"raw":rows,"summary":summary])
        }
        for (n,page) in pages.enumerated() {
            let mode=modes[n];zoom[mode]=[];scroll[mode]=[];accuracy[mode]=[]
            for dpi in novel {_=try page.engine.render(9,dpi);var row=try page.render(dpi);row["dpi"]=dpi;zoom[mode]!.append(row)}
            _=try page.render(300);let before=page.counters(),requests=page.engine.requests
            for i in -warmups..<samples {let start=now(),(meta,_,_)=try page.scroll(Double((i+warmups)%60)/59);if i>=0 {scroll[mode]!.append(["frame_ms":now()-start,"presentation":meta])}}
            let scrollAfter=page.counters()
            guard requests==page.engine.requests,metric(before["gpu"] as! [String:Any],"dispatches")==metric(scrollAfter["gpu"] as! [String:Any],"dispatches") else {throw fail("Scroll rendered fresh pixels")}
            for dpi in dpis+novel {accuracy[mode]!.append(try page.accuracy(dpi))}
            cases.append(["label":"Document \(index+1)","mode":mode,"input_sha256":digest(pdf),"input_file_read_ms":fileMs,"fallback":page.fallback as Any? ?? NSNull(),"opening_raw":opening[mode]!,"opening":distribution(opening[mode]!.map{metric($0,"first_display_ms")}),"static":staticCases.map{["dpi":$0["dpi"]!,"raw":($0["raw"] as! [String:[[String:Any]]])[mode]!,"summary":($0["summary"] as! [String:[String:[String:Double]]])[mode]!] as [String:Any]},"novel_zoom_raw":zoom[mode]!,"novel_zoom":distribution(zoom[mode]!.map{metric($0,"frame_ms")}),"scroll_raw":scroll[mode]!,"scroll":distribution(scroll[mode]!.map{metric($0,"frame_ms")}),"scroll_before":before,"scroll_after":scrollAfter,"accuracy":accuracy[mode]!])
        }
    }
    let report:[String:Any]=["device":context.device.name,"unified_memory":context.device.hasUnifiedMemory,"application_initialization_ms":context.initializationMs,"samples":samples,"warmups":warmups,"cold_samples":cold,"clock":"Native request through drawable presentation-or-discard callback; no Node relay. Document-cold opening, application/presentation warm; file read reported separately. Not scanout.","cases":cases,"failures":[],"presentations":context.screen.frames,"discarded":context.screen.droppedFrames]
    let data=try JSONSerialization.data(withJSONObject:report,options:[.sortedKeys]);try FileHandle.standardOutput.write(contentsOf:data)
}
@main struct SharedMain {
    @MainActor static func main() {do {try integratedStudy()} catch {FileHandle.standardError.write(Data("Shared host failed: \(error)\n".utf8));exit(1)}}
}
