// G01 public CoreGraphics APIs; PDFKit is not this baseline.
import Foundation
import CoreGraphics
import CryptoKit

enum ProbeFailure: Error { case malformed, limit, operation }

func dictionary(_ parent: CGPDFDictionaryRef, _ key: String) -> CGPDFDictionaryRef? {
    var result: CGPDFDictionaryRef?
    return CGPDFDictionaryGetDictionary(parent, key, &result) ? result : nil
}

func string(_ parent: CGPDFDictionaryRef, _ key: String) -> String? {
    var result: CGPDFStringRef?
    guard CGPDFDictionaryGetString(parent, key, &result), let result,
          let text = CGPDFStringCopyTextString(result) else { return nil }
    return text as String
}

func attachments(_ document: CGPDFDocument) throws -> [[String: String]] {
    guard let catalog = document.catalog, let names = dictionary(catalog, "Names"),
          let files = dictionary(names, "EmbeddedFiles") else { return [] }
    var pending = [files]
    var visited = 0
    var result: [[String: String]] = []
    while let node = pending.popLast() {
        visited += 1
        guard visited <= 128 else { throw ProbeFailure.limit }
        var kids: CGPDFArrayRef?
        if CGPDFDictionaryGetArray(node, "Kids", &kids), let kids {
            let count = CGPDFArrayGetCount(kids)
            guard count <= 128, pending.count + count <= 128 else { throw ProbeFailure.limit }
            for i in 0..<count {
                var child: CGPDFDictionaryRef?
                guard CGPDFArrayGetDictionary(kids, i, &child), let child else { throw ProbeFailure.malformed }
                pending.append(child)
            }
        }
        var entries: CGPDFArrayRef?
        if CGPDFDictionaryGetArray(node, "Names", &entries), let entries {
            let count = CGPDFArrayGetCount(entries)
            guard count % 2 == 0, count <= 256, result.count + count / 2 <= 128 else { throw ProbeFailure.limit }
            for i in stride(from: 1, to: count, by: 2) {
                var specification: CGPDFDictionaryRef?
                guard CGPDFArrayGetDictionary(entries, i, &specification), let specification,
                      let name = string(specification, "UF") ?? string(specification, "F"),
                      let embedded = dictionary(specification, "EF") else { throw ProbeFailure.malformed }
                var stream: CGPDFStreamRef?
                guard CGPDFDictionaryGetStream(embedded, "F", &stream), let stream else { throw ProbeFailure.malformed }
                var format = CGPDFDataFormat.raw
                guard let bytes = CGPDFStreamCopyData(stream, &format) else { throw ProbeFailure.malformed }
                guard format == .raw, CFDataGetLength(bytes) <= 65536 else { throw ProbeFailure.limit }
                let digest = SHA256.hash(data: bytes as Data).map { String(format: "%02x", $0) }.joined()
                result.append(["name": name, "sha256": digest])
            }
        }
    }
    return result.sorted { $0["name", default: ""] < $1["name", default: ""] }
}

func read(_ document: CGPDFDocument, _ operation: String) throws -> Any {
    switch operation {
    case "pages":
        var sizes: [[Double]] = []
        for index in 0..<document.numberOfPages {
            guard let page = document.page(at: index + 1) else { throw ProbeFailure.malformed }
            let box = page.getBoxRect(.cropBox)
            let rotated = abs(page.rotationAngle % 180) == 90
            sizes.append(rotated ? [box.height, box.width] : [box.width, box.height])
        }
        return ["count": document.numberOfPages, "sizes": sizes] as [String: Any]
    case "metadata":
        guard let info = document.info else { return ["Title": "", "Author": ""] }
        return ["Title": string(info, "Title") ?? "", "Author": string(info, "Author") ?? ""]
    case "attachments": return try attachments(document)
    default: throw ProbeFailure.operation
    }
}

let args = Array(CommandLine.arguments.dropFirst())
var response: [String: Any]
if args == ["--version"] {
    response = ["engine": "Apple CoreGraphics", "wrapper": "CoreGraphics public API via Swift",
                "system": ProcessInfo.processInfo.operatingSystemVersionString]
} else if args.count == 2 {
    if let document = CGPDFDocument(URL(fileURLWithPath: args[1]) as CFURL) {
        do { response = ["schema": 1, "status": "ok", "value": try read(document, args[0])] }
        catch { response = ["schema": 1, "status": "adapter-error", "reason": "CoreGraphics adapter read failed"] }
    } else { response = ["schema": 1, "status": "rejected", "reason": "CGPDFDocument open failed"] }
} else { response = ["schema": 1, "status": "adapter-error", "reason": "expected OPERATION SYNTHETIC_PDF"] }
let data = try JSONSerialization.data(withJSONObject: response, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data([10]))
