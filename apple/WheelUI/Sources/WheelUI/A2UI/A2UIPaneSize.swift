import SwiftUI

private struct A2UIViewportKey: EnvironmentKey { static let defaultValue = CGSize(width: 640, height: 640) }
extension EnvironmentValues {
    /// The pane's own size, which the page's rows and lists fit themselves to.
    var a2uiPane: CGSize {
        get { self[A2UIViewportKey.self] }
        set { self[A2UIViewportKey.self] = newValue }
    }
    var a2uiViewport: CGFloat { a2uiPane.height }
}
