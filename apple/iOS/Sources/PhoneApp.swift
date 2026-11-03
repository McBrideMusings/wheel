import SwiftUI
import WheelKit
import WheelUI

@main
struct PhoneApp: App {
    @UIApplicationDelegateAdaptor(PhoneAppDelegate.self) private var delegate
    @State private var model = PhoneModel()

    init() { UILog.sink = { Log.info($0) } }

    var body: some Scene {
        WindowGroup {
            Group {
                if model.isPaired && !model.certificateMismatch && !model.credentialRefused {
                    SpaceCarousel(model: model)
                } else {
                    PairView(model: model)
                }
            }
            .task {
                model.start()
                PushCenter.shared.listen { space, pane in model.focus(space: space, pane: pane) }
                PushCenter.shared.onToken = { model.pushToken($0) }
                PushCenter.shared.start()
            }
        }
    }
}
