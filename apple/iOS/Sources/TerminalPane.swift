import GhosttyTerminal
import SwiftUI
import WheelKit

/// One terminal pane's Ghostty surface on the phone, and the in-memory session the daemon's bytes
/// are fed to. The same arrangement as the Mac's `PaneTerminal`.
@MainActor
final class PhoneTerminal: ObservableObject {
    let term: String
    /// The plugin pane whose page draws this surface (`Terminal{pane}`, ADR-0042); nil for the terminal's own page.
    let host: String?
    let view: TerminalViewState
    let session: InMemoryTerminalSession
    /// The daemon is streaming this terminal to this connection.
    @Published var attached = false
    /// The grid size last sent to the daemon; cleared on every attach so the phone's size goes again.
    var size: (cols: Int, rows: Int)?
    /// The page's laid-out size in points and the display scale, from SwiftUI.
    var layout: (size: CGSize, scale: CGFloat)?
    /// The surface's latest grid, held until `layout` says the surface has its final frame.
    var reported: InMemoryTerminalViewport?
    /// The most a page exceeds its surface by on one axis, besides the cell remainder: the
    /// surface's padding (20 pt wide, 16 pt tall), rounded up to one value for both axes.
    static let paddingPoints: CGFloat = 24

    init(term: String, host: String? = nil, controller: TerminalController, model: PhoneModel) {
        self.term = term
        self.host = host
        view = TerminalViewState(controller: controller)
        // The session's callbacks name the surface, so the model tells a page's surface from an embed's of one term.
        let back = SurfaceRef()
        session = InMemoryTerminalSession(
            write: { [weak model] data in
                Task { @MainActor in if let surface = back.surface { model?.typed(surface, data) } }
            },
            resize: { [weak model] vp in
                Task { @MainActor in if let surface = back.surface { model?.resized(surface, vp) } }
            },
            suppressesPixelOnlyResizes: true
        )
        view.configuration = TerminalSurfaceOptions(backend: .inMemory(session))
        back.surface = self
    }
}

/// The surface a session's callbacks report for, set once the surface exists.
@MainActor private final class SurfaceRef {
    weak var surface: PhoneTerminal?
}

/// A terminal pane's page: the shell's screen, with the keyboard's key bar (esc, tab, ctrl, arrows)
/// above the on-screen keyboard.
struct TerminalPage: View {
    let model: PhoneModel
    @ObservedObject var terminal: PhoneTerminal
    @ObservedObject var termState: TerminalViewState

    @Environment(\.displayScale) private var scale
    @State private var ctrlArmed = false

    private func laidOut(_ size: CGSize) {
        terminal.layout = (size, scale)
        model.settle(terminal)
    }

    var body: some View {
        TerminalSurfaceView(context: termState)
            .background(GeometryReader { geo in
                Color.clear
                    .onChange(of: geo.size, initial: true) { _, _ in laidOut(geo.size) }
                    .onChange(of: scale) { _, _ in laidOut(geo.size) }
            })
            // The bar is SwiftUI's own, above the keyboard, so the page's size already leaves it room:
            // the library's accessory bar sits outside the layout and covered the last rows in landscape.
            .safeAreaInset(edge: .bottom, spacing: 0) {
                KeyBar(termState: termState, ctrlArmed: $ctrlArmed)
            }
            .onAppear { termState.inputAccessoryItems = [] }
            .overlay(alignment: .center) {
                if !terminal.attached {
                    ProgressView(model.connection == .connected ? "Attaching…" : "Connecting…")
                        .padding()
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
                }
            }
    }
}

/// esc, tab, ctrl and the arrows: what the on-screen keyboard lacks. Ctrl is sticky for the next key,
/// as the library's own bar has it.
private struct KeyBar: View {
    @ObservedObject var termState: TerminalViewState
    @Binding var ctrlArmed: Bool

    var body: some View {
        // A column narrower than the keys (a terminal inside a plugin page) scrolls them sideways instead of
        // widening the page past the screen.
        ViewThatFits(in: .horizontal) {
            keys(spacer: true)
            ScrollView(.horizontal, showsIndicators: false) { keys(spacer: false) }
        }
        .background(.bar)
    }

    private func keys(spacer: Bool) -> some View {
        HStack(spacing: 6) {
            key("esc") { _ = termState.sendKey(.escape) }
            key("tab") { _ = termState.sendKey(.tab) }
            key("ctrl", on: ctrlArmed) { toggleCtrl() }
            if spacer { Spacer(minLength: 0) }
            key("←") { _ = termState.sendKey(.arrowLeft) }
            key("↓") { _ = termState.sendKey(.arrowDown) }
            key("↑") { _ = termState.sendKey(.arrowUp) }
            key("→") { _ = termState.sendKey(.arrowRight) }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
    }

    private func key(_ label: String, on: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.system(.body, design: .monospaced))
                .frame(minWidth: 40, minHeight: 32)
                .background(on ? Color.accentColor.opacity(0.35) : Color.secondary.opacity(0.18), in: RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
    }

    private func toggleCtrl() {
        guard let view = termState.attachedPlatformView else { return }
        view.setStickyModifierChangeHandler { [weak view] in
            ctrlArmed = view?.stickyActivation(for: .ctrl) != .inactive
        }
        view.toggleStickyModifier(.ctrl)
        Log.info("ctrl \(view.stickyActivation(for: .ctrl))")
    }
}
