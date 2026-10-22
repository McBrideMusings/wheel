import SwiftUI
import WheelKit

@main
struct PhoneApp: App {
    @State private var model = PhoneModel()

    var body: some Scene {
        WindowGroup {
            Group {
                if model.isPaired {
                    SpaceCarousel(model: model)
                } else {
                    PairView(model: model)
                }
            }
            .task { model.start() }
        }
    }
}
