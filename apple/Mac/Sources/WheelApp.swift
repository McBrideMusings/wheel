import SwiftUI

@main
struct WheelApp: App {
    @StateObject private var model = AppModel()
    @State private var started = false
    #if DEBUG
    @State private var debug: DebugServer?
    #endif

    var body: some Scene {
        // One window: the lead card beside the active space's grid.
        Window("Wheel", id: "main") {
            ContentView(model: model)
                .task {
                    guard !started else { return }
                    started = true
                    // WHEEL_APPEARANCE=light|dark pins the appearance, so both can be checked on one machine.
                    switch ProcessInfo.processInfo.environment["WHEEL_APPEARANCE"] {
                    case "light": NSApp.appearance = NSAppearance(named: .aqua)
                    case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
                    default: break
                    }
                    #if DEBUG
                    let server = DebugServer(model: model)
                    server.start()
                    debug = server
                    #endif
                    model.start()
                }
        }
    }
}
