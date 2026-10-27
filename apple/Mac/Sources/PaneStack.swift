import SwiftUI
import WheelKit

/// A space's panes as a stack of rounded rectangles, one per pane up to `SpaceInfo.stackLimit`,
/// each offset to the right of the one in front; the front layer carries the first pane's kind.
struct PaneStack: View {
    let space: SpaceInfo
    var size: CGFloat = 14

    private let step: CGFloat = 3

    var body: some View {
        let layers = space.stackLayers
        let count = space.root.panes.count
        ZStack(alignment: .leading) {
            ForEach(Array((0..<layers).reversed()), id: \.self) { index in
                RoundedRectangle(cornerRadius: 3)
                    .fill(.background)
                    .overlay(RoundedRectangle(cornerRadius: 3).stroke(lineWidth: 1))
                    .frame(width: size, height: size)
                    .offset(x: CGFloat(index) * step)
            }
            Image(systemName: space.firstPaneKind.icon)
                .font(.system(size: size * 0.6))
                .frame(width: size, height: size)
        }
        .frame(width: size + CGFloat(layers - 1) * step, height: size, alignment: .leading)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(count) \(count == 1 ? "pane" : "panes")")
    }
}
