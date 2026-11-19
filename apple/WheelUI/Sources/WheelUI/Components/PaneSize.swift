import SwiftUI

private struct PaneViewportKey: EnvironmentKey { static let defaultValue = CGSize(width: 640, height: 640) }
extension EnvironmentValues {
    /// The pane's own size, which the page's rows and lists fit themselves to.
    var paneSize: CGSize {
        get { self[PaneViewportKey.self] }
        set { self[PaneViewportKey.self] = newValue }
    }
    var paneViewport: CGFloat { paneSize.height }
}
