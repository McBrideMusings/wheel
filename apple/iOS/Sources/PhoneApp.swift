import SwiftUI
import WheelKit
import WheelUI

@main
struct PhoneApp: App {
    @UIApplicationDelegateAdaptor(PhoneAppDelegate.self) private var delegate
    @State private var servers = ServerList()

    init() { UILog.sink = { Log.info($0) } }

    var body: some Scene {
        WindowGroup {
            Group {
                if let model = servers.shown, !model.certificateMismatch, !model.credentialRefused {
                    SpaceCarousel(model: model, servers: servers)
                } else {
                    // A server that refuses this phone is paired again in place; with none paired, the first one is.
                    PairView(model: servers.shown ?? servers.adding, servers: servers)
                }
            }
            // Voice mode's turns and its audio's own stops reach the shown server's session wherever the pane is.
            .onChange(of: Voice.shared.said) { _, said in servers.shown?.voice.heard(said) }
            .onChange(of: Voice.shared.phase) { _, phase in servers.shown?.voice.audioStopped(phase: phase, problem: Voice.shared.problem) }
            .onDisappear { servers.shown?.voice.end() }
            .task {
                servers.start()
                PushCenter.shared.listen(onOpen: { space, pane in servers.focus(space: space, pane: pane) })
                PushCenter.shared.onToken = { servers.pushToken($0) }
                // Debug builds launched with `WHEEL_NOTIFICATIONS=off` never ask, so a check's
                // screenshots carry no permission alert.
                #if DEBUG
                let ask = ProcessInfo.processInfo.environment["WHEEL_NOTIFICATIONS"] != "off"
                #else
                let ask = true
                #endif
                if ask { PushCenter.shared.start() }
            }
        }
    }
}
