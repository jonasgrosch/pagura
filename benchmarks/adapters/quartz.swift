// External platform reference. No Apple code or native binding enters Pagura.
import Foundation
import CoreGraphics

enum Failure: Error { case input, render }
var args = CommandLine.arguments
var password: String? = nil
if let index = args.firstIndex(of: "--password") {
    guard index + 1 < args.count else { throw Failure.input }
    password = args[index + 1]
    args.removeSubrange(index...(index + 1))
}
if args.count == 2 && args[1] == "--version" {
    print("Apple CoreGraphics; " + ProcessInfo.processInfo.operatingSystemVersionString)
} else {
    guard (args.count == 5 || args.count == 6), let dpi = Double(args[3]), dpi.isFinite, dpi > 0,
          let first = Int(args[4]), let last = Int(args.last ?? ""),
          first >= 0, first <= last, last < 100_000 else { throw Failure.input }
    let started = DispatchTime.now().uptimeNanoseconds
    guard let document = CGPDFDocument(URL(fileURLWithPath: args[1]) as CFURL) else { throw Failure.input }
    if let password = password {
        guard document.unlockWithPassword(password) else { throw Failure.input }
    } else if document.isEncrypted && !document.isUnlocked {
        guard document.unlockWithPassword("") else { throw Failure.input }
    }
    let indices = 0..<document.numberOfPages
    guard indices.contains(last) else { throw Failure.input }
    let opened = DispatchTime.now().uptimeNanoseconds
    var stages: [String] = []
    for index in first...last {
    let renderStarted = DispatchTime.now().uptimeNanoseconds
    guard let page = document.page(at: index + 1) else { throw Failure.input }
    let box = page.getBoxRect(.cropBox)
    let rotated = abs(page.rotationAngle % 180) == 90
    let pageWidth = rotated ? box.height : box.width
    let pageHeight = rotated ? box.width : box.height
    let scale = dpi / 72
    let pixelWidth = ceil(pageWidth * scale)
    let pixelHeight = ceil(pageHeight * scale)
    guard pixelWidth.isFinite, pixelHeight.isFinite,
          pixelWidth > 0, pixelHeight > 0, pixelWidth * pixelHeight <= 16_000_000
          else { throw Failure.render }
    let width = Int(pixelWidth), height = Int(pixelHeight)
    guard
          let space = CGColorSpace(name: CGColorSpace.sRGB),
          let context = CGContext(data: nil, width: width, height: height,
              bitsPerComponent: 8, bytesPerRow: width * 4, space: space,
              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
          else { throw Failure.render }
    context.setFillColor(red: 1, green: 1, blue: 1, alpha: 1)
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    // CGPDFPageGetDrawingTransform fits by scaling down only (SDK contract).
    // Map the page in points, then apply DPI explicitly, including enlargement.
    context.scaleBy(x: scale, y: scale)
    context.concatenate(page.getDrawingTransform(.cropBox,
        rect: CGRect(x: 0, y: 0, width: pageWidth, height: pageHeight), rotate: 0, preserveAspectRatio: true))
    context.drawPDFPage(page)
    let rendered = DispatchTime.now().uptimeNanoseconds
    guard let raw = context.data else { throw Failure.render }
    let pixels = raw.bindMemory(to: UInt8.self, capacity: width * height * 4)
    var output = Data("RGBA8 \(width) \(height) premultiplied\n".utf8)
    output.append(pixels, count: width * height * 4)
    let target = args.count == 6
        ? URL(fileURLWithPath: args[2]).appendingPathComponent(String(format: "page-%04d.ppm", index + 1))
        : URL(fileURLWithPath: args[2])
    try output.write(to: target)
    let emitted = DispatchTime.now().uptimeNanoseconds
    stages.append("{\"page\":\(index),\"render_ns\":\(rendered - renderStarted),\"emit_ns\":\(emitted - rendered)}")
    }
    let total = DispatchTime.now().uptimeNanoseconds - started
    print("{\"schema\":1,\"open_ns\":\(opened - started),\"pages\":[\(stages.joined(separator: ","))],\"total_ns\":\(total)}")
}
