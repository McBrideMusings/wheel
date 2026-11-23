import BenchCore
import Foundation

// bench <spec> <clipsDir> <modelsDir> <out.json>
let a = CommandLine.arguments
guard a.count == 5 else { print("usage: bench <spec> <clipsDir> <modelsDir> <out.json>"); exit(2) }
try await run(spec: a[1], clipsDir: URL(fileURLWithPath: a[2]), modelsDir: URL(fileURLWithPath: a[3]), out: URL(fileURLWithPath: a[4])) { print($0) }
