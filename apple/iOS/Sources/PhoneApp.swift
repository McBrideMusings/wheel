import SwiftUI
import WheelKit
import WheelUI

@main
struct PhoneApp: App {
    @State private var model = PhoneModel()

    init() { UILog.sink = { Log.info($0) } }

    var body: some Scene {
        WindowGroup {
            Group {
                if model.isPaired && !model.certificateMismatch {
                    SpaceCarousel(model: model)
                } else {
                    PairView(model: model)
                }
            }
            .task { model.start() }
        }
    }
}
