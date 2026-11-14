import SwiftUI
#if os(macOS)
import AppKit
#else
import UIKit
#endif

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
                    Banner(text: message, detail: pane.diagnostics, tone: palette.bad, id: "a2ui.backend-down")
                    Button("Try again") { pane.start() }.padding(8).accessibilityIdentifier("a2ui.retry")
                }
            } else if let notice = pane.notice {
                Banner(text: notice, detail: pane.diagnostics, tone: palette.warn, id: "a2ui.notice")
            }
            if let surface = pane.surface {
                GeometryReader { geo in
                    Group {
                        if surface.fillsPane {
                            A2UISurfaceView(surface: surface)
                                .frame(width: geo.size.width, height: geo.size.height, alignment: .top)
                        } else {
                            ScrollView {
                                A2UISurfaceView(surface: surface)
                                    .frame(maxWidth: .infinity, alignment: .top)
                            }
                        }
                    }
                    .environment(\.a2uiPane, geo.size)
                    .overlay { A2UIPresentedModals(surface: surface) }
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

    /// A message above the page: its text can be selected, and the button copies the text with the pane's details.
    private struct Banner: View {
        let text: String
        let detail: String
        let tone: Color
        let id: String
        @State private var copied = false
        var body: some View {
            HStack(alignment: .top, spacing: 8) {
                Text(text)
                    .font(.system(size: 12))
                    .foregroundStyle(tone)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button {
                    let report = text + "\n\n" + detail
                    #if os(macOS)
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(report, forType: .string)
                    #else
                    UIPasteboard.general.string = report
                    #endif
                    copied = true
                    Task { try? await Task.sleep(nanoseconds: 1_500_000_000); copied = false }
                } label: {
                    Image(systemName: copied ? "checkmark" : "doc.on.doc").frame(width: 16, height: 16)
                }
                .buttonStyle(.borderless)
                .help("Copy the message and the pane's details")
                .accessibilityLabel("Copy error details")
                .accessibilityIdentifier("\(id).copy")
            }
            .padding(8)
            .background(tone.opacity(0.08))
            .accessibilityIdentifier(id)
        }
    }
}

extension SurfaceModel {
    /// A Table or a vertical List with no `frame` modifier fills the pane, so the page around it stays put and that
    /// component scrolls itself; any other page scrolls as a whole. Only a component the page now shows counts: the
    /// surface keeps every component it was ever sent.
    var fillsPane: Bool {
        func scrollsItself(_ n: ComponentNode) -> Bool {
            switch n.instance.type {
            case "Table": true
            case "List": n.instance.properties["direction"]?.stringValue != "horizontal"
            default: false
            }
        }
        func shows(_ n: ComponentNode) -> Bool {
            (scrollsItself(n) && !(n.instance.properties["modifiers"]?.arrayValue ?? []).contains { $0.dictionaryValue?["frame"] != nil })
                || n.children.contains(where: shows)
        }
        return root.map(shows) ?? false
    }
}
