import AppKit
// tab-ink <window shot png> <geometry json> <window id prefix>: for each tab in the window's
// `rows` (GET /geometry, screen points, bottom-left origin), the number of drawn pixels inside its
// frame: pixels with a channel above 110, which the tab's name and marks are and the title row's
// ground (about 40) is not. Prints `<space> <count>` per tab. A tab the window does not draw
// counts 0. The shot is /window/snapshot?frame=1 at the screen's scale.
let args = CommandLine.arguments
let rep = NSBitmapImageRep(data: try! Data(contentsOf: URL(fileURLWithPath: args[1])))!
let geo = try! JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: args[2]))) as! [String: Any]
let windows = geo["windows"] as! [String: [String: Any]]
let window = windows.first { $0.key.hasPrefix(args[3]) }!.value
let frame = window["frame"] as! [Double]
let scale = Double(rep.pixelsWide) / frame[2]
let rows = window["rows"] as? [String: [Double]] ?? [:]
for (space, r) in rows.sorted(by: { $0.value[0] < $1.value[0] }) {
    let x0 = Int((r[0] - frame[0]) * scale), w = Int(r[2] * scale)
    let y0 = Int((frame[1] + frame[3] - (r[1] + r[3])) * scale), h = Int(r[3] * scale)
    var ink = 0
    for y in max(0, y0)..<min(rep.pixelsHigh, y0 + h) {
        for x in max(0, x0)..<min(rep.pixelsWide, x0 + w) {
            guard let c = rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
            if max(c.redComponent, c.greenComponent, c.blueComponent) > 110.0 / 255 { ink += 1 }
        }
    }
    print(space, ink)
}
