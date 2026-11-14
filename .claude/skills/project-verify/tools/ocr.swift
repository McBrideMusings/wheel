// Print the text Vision reads in an image, one recognised line per output line: `swift ocr.swift <image.png>`.
// The checks use it to prove text reached the screen (a simulator screenshot), not only the log.
import AppKit
import Vision

guard CommandLine.arguments.count == 2, let image = NSImage(contentsOfFile: CommandLine.arguments[1]),
      let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write("usage: ocr <image>\n".data(using: .utf8)!)
    exit(2)
}
let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = false
try VNImageRequestHandler(cgImage: cg).perform([request])
for line in request.results ?? [] {
    if let text = line.topCandidates(1).first?.string { print(text) }
}
