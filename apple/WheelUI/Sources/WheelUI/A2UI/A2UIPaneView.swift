import A2UISwiftCore
import A2UISwiftUI
import SwiftUI

/// The native body of a pane whose MCP server speaks A2UI: the surface, with the last refusal of
/// an action above it and, when the server stops answering, its message in place of a blank pane.
public struct A2UIPaneView: View {
    let pane: A2UIPane
    @Environment(\.colorScheme) private var scheme

    public init(pane: A2UIPane) { self.pane = pane }

    public var body: some View {
        let palette = WheelPalette(dark: scheme == .dark)
        VStack(spacing: 0) {
            if case let .failed(message) = pane.state {
                VStack(alignment: .leading, spacing: 0) {
                    Banner(text: message, tone: palette.bad, id: "a2ui.backend-down")
                    Button("Try again") { pane.start() }.padding(8).accessibilityIdentifier("a2ui.retry")
                }
            } else if let notice = pane.notice {
                Banner(text: notice, tone: palette.warn, id: "a2ui.notice")
            }
            if let surface = pane.surface {
                GeometryReader { geo in
                    ScrollView {
                        A2UISurfaceView(viewModel: styled(surface, palette), catalog: WheelA2UICatalog())
                            .padding(-16) // the surface view pads itself by 16; the page sets its own margins
                            .frame(maxWidth: .infinity, alignment: .top)
                    }
                    .environment(\.a2uiPane, geo.size)
                }
            } else if case .failed = pane.state {
                Spacer()
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(palette.bg)
        .accessibilityIdentifier("a2ui.pane")
    }

    /// The theme goes on the view model: the surface view re-injects it from there and overwrites
    /// a modifier set outside.
    private func styled(_ surface: SurfaceViewModel, _ palette: WheelPalette) -> SurfaceViewModel {
        surface.a2uiStyle = themedStyle(palette, leaf: 0)
        return surface
    }

    private struct Banner: View {
        let text: String
        let tone: Color
        let id: String
        var body: some View {
            Text(text)
                .font(.system(size: 12))
                .foregroundStyle(tone)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(8)
                .background(tone.opacity(0.08))
                .accessibilityIdentifier(id)
        }
    }
}
