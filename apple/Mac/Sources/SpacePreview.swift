import AppKit
import SwiftUI
import WheelKit
import WheelUI

/// The hover preview of a menu bar row: the space's pane grid as the panes look, each pane with
/// its status dot and last line, and under it the processes its panes started. It sits in a
/// window of its own beside the popover and lets every click through, so it never takes the
/// pointer from the row under it.
@MainActor
final class SpacePreviewPanel {
    static let width: CGFloat = 360
    private let window: NSWindow
    private let host: NSHostingView<SpacePreviewView>

    /// Which side of the popover the panel is on, and where it is, for the state query.
    private(set) var side = "none"
    var frame: CGRect { window.frame }
    var isShown: Bool { window.isVisible }
    var contentView: NSView? { window.contentView }

    init(model: AppModel) {
        host = NSHostingView(rootView: SpacePreviewView(model: model, space: ""))
        window = NSWindow(contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.backgroundColor = .clear
        window.isOpaque = false
        window.hasShadow = true
        window.ignoresMouseEvents = true
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        window.contentView = host
    }

    /// Show `space`'s preview beside `popover`, level with `row` (screen coordinates): on the left
    /// of the popover, or on its right when the screen has no room on the left.
    func show(model: AppModel, space: String, row: CGRect, popover: NSWindow) {
        host.rootView = SpacePreviewView(model: model, space: space)
        let size = host.fittingSize
        let screen = popover.screen?.visibleFrame ?? NSScreen.main?.visibleFrame ?? .zero
        let gap: CGFloat = 8
        let leftX = popover.frame.minX - gap - size.width
        let onLeft = leftX >= screen.minX
        let x = onLeft ? leftX : popover.frame.maxX + gap
        // `row` is in AppKit's coordinates: y grows upward, so the panel's top is the row's top.
        let top = min(row.maxY, screen.maxY)
        let y = max(top - size.height, screen.minY)
        side = onLeft ? "left" : "right"
        window.level = NSWindow.Level(rawValue: popover.level.rawValue + 1)
        window.setFrame(NSRect(x: x, y: y, width: size.width, height: size.height), display: true)
        window.orderFrontRegardless()
    }

    func hide() {
        window.orderOut(nil)
        // An empty space stops the view's polling.
        host.rootView = SpacePreviewView(model: host.rootView.model, space: "")
        side = "none"
    }
}

extension AppModel {
    /// The pointer entered or left `space`'s popover row: its place buttons and its preview follow.
    func rowHover(_ space: String, inside: Bool) {
        if inside {
            popoverHover = space
            previewHover(space)
        } else if popoverHover == space {
            popoverHover = nil
            previewLeave()
        }
    }

    /// The pointer is over `space`'s popover row. The preview opens after 250 ms, or at once when
    /// another row's preview is already open, and swaps to this row's.
    func previewHover(_ space: String) {
        previewTask?.cancel()
        let delay: Duration = previewSpace == nil ? .milliseconds(250) : .zero
        previewTask = Task { @MainActor [weak self] in
            if delay > .zero { try? await Task.sleep(for: delay) }
            guard !Task.isCancelled else { return }
            self?.openPreview(space)
        }
    }

    /// The pointer left a row: the preview closes unless another row is entered within 100 ms.
    func previewLeave() {
        previewTask?.cancel()
        guard previewSpace != nil else { return }
        previewTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(100))
            guard !Task.isCancelled else { return }
            self?.closePreview()
        }
    }

    func closePreview() {
        previewTask?.cancel()
        guard previewSpace != nil || previewPanel?.isShown == true else { return }
        Log.info("preview: closed (was \(previewSpace ?? "none"))")
        previewSpace = nil
        previewPanel?.hide()
    }

    private func openPreview(_ space: String) {
        guard let pop = MenuBarWindow.current, pop.isVisible else {
            Log.info("preview: \(space) not opened, the popover is not open")
            return
        }
        guard let frame = popoverRowFrames.values.lazy.compactMap({ $0[space] }).first else {
            Log.info("preview: \(space) not opened, its row has no frame")
            return
        }
        let panel = previewPanel ?? SpacePreviewPanel(model: self)
        previewPanel = panel
        requestProcs(space)
        previewSpace = space
        panel.show(model: self, space: space, row: screenRect(frame, in: pop), popover: pop)
        Log.info("preview: opened \(space) on the \(panel.side) of the popover at \(panel.frame)")
    }

    /// The last thing a pane shows, as one line: an agent's last reply or tool call, the last
    /// line of a terminal, a browser's address, a web pane's artifact.
    func lastLine(ofPane pane: String, kind: PaneKind) -> String? {
        switch kind {
        case .empty: return nil
        case let .agent(session, _):
            guard let rows = chats[session]?.transcript.rows else { return nil }
            for row in rows.reversed() {
                switch row.kind {
                case let .agent(text): if let line = Self.lastLine(of: text) { return line }
                case let .user(text, _): if let line = Self.lastLine(of: text) { return "> " + line }
                case let .tool(title, _, _, _): return title
                case let .error(text), let .notice(text): if let line = Self.lastLine(of: text) { return line }
                case .thought, .plan, .mode: continue
                }
            }
            return nil
        case let .terminal(term, _):
            return terminals[term]?.viewportText.flatMap(Self.lastLine(of:))
        case let .browser(url): return url
        case let .web(artifact): return artifact.name
        case .unknown: return nil
        }
    }

    static func lastLine(of text: String) -> String? {
        text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .last { !$0.isEmpty }
    }

    /// What the preview of `space` shows, for the state query.
    func previewPanes(of space: String) -> [(pane: String, kind: String, status: String?, last: String?)] {
        guard let info = spaces.first(where: { $0.id == space }) else { return [] }
        return info.root.panes.map { pane in
            (pane.id, pane.kind.name, info.paneStatuses[pane.id].map { $0.rank.rawValue }, lastLine(ofPane: pane.id, kind: pane.kind))
        }
    }
}

struct SpacePreviewView: View {
    @ObservedObject var model: AppModel
    let space: String

    private static let gridHeight: CGFloat = 210
    private let tick = Timer.publish(every: 2, on: .main, in: .common).autoconnect()

    var body: some View {
        // The chats and terminals are not published, so a once-a-second timeline redraws the lines; a 2 s timer refreshes the processes.
        TimelineView(.periodic(from: .now, by: 1)) { _ in
            content
        }
        .onReceive(tick) { _ in if !space.isEmpty { model.requestProcs(space) } }
    }

    @ViewBuilder
    private var content: some View {
        if let info = model.spaces.first(where: { $0.id == space }) {
            VStack(alignment: .leading, spacing: 8) {
                Text(info.name).font(.callout.weight(.semibold)).lineLimit(1)
                GeometryReader { geo in
                    let grid = WheelKit.GridLayout(info.root, in: CGRect(origin: .zero, size: geo.size))
                    ForEach(grid.panes) { pane in
                        PreviewPane(model: model, info: info, pane: pane.id, kind: pane.kind)
                            .frame(width: pane.frame.width, height: pane.frame.height)
                            .position(x: pane.frame.midX, y: pane.frame.midY)
                    }
                }
                .frame(height: Self.gridHeight)
                PreviewProcesses(procs: model.procs[space] ?? [])
            }
            .padding(10)
            .frame(width: SpacePreviewPanel.width)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
            .accessibilityHidden(true)
        }
    }
}

/// One pane of the preview: its kind and name, its status dot, and its last line.
private struct PreviewPane: View {
    @ObservedObject var model: AppModel
    let info: SpaceInfo
    let pane: String
    let kind: PaneKind

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 5) {
                Image(systemName: kind.icon).font(.system(size: 10)).foregroundStyle(.secondary)
                Text(title).font(.caption2.weight(.semibold)).lineLimit(1)
                Spacer(minLength: 0)
                StatusDot(status: info.paneStatuses[pane], shown: model.statusDots)
            }
            if let last = model.lastLine(ofPane: pane, kind: kind) {
                Text(last)
                    .font(.system(size: 10, design: isTerminal ? .monospaced : .default))
                    .foregroundStyle(.secondary)
                    .lineLimit(4)
                    .truncationMode(.tail)
            }
            Spacer(minLength: 0)
        }
        .padding(6)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 5))
        .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(pane == info.focus ? Color.accentColor.opacity(0.7) : Color.secondary.opacity(0.35)))
        .clipShape(RoundedRectangle(cornerRadius: 5))
    }

    private var isTerminal: Bool { if case .terminal = kind { true } else { false } }

    private var title: String {
        switch kind {
        case let .agent(_, agent): agent
        case let .terminal(_, agent): agent?.name ?? "terminal"
        default: kind.name
        }
    }
}

/// The processes the space's panes started: up to four by name and CPU, then how many more.
private struct PreviewProcesses: View {
    let procs: [ProcInfo]
    private static let limit = 4

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if procs.isEmpty {
                Text("No processes running").font(.caption).foregroundStyle(.secondary)
            } else {
                ForEach(procs.prefix(Self.limit)) { proc in
                    HStack(spacing: 6) {
                        Image(systemName: "gearshape").font(.system(size: 9)).foregroundStyle(.secondary)
                        Text(proc.name).font(.caption).lineLimit(1)
                        Spacer(minLength: 4)
                        Text(String(format: "%.0f%%", proc.cpu)).font(.caption).monospacedDigit().foregroundStyle(.secondary)
                    }
                }
                if procs.count > Self.limit {
                    Text("+\(procs.count - Self.limit) more").font(.caption2).foregroundStyle(.secondary)
                }
            }
        }
    }
}
