import AppKit
import SwiftUI
import WheelKit

/// A new space `pane_move` is about to make, and where the window layout puts it once the daemon
/// reports it: the first new space that appears within ten seconds.
struct PendingNewSpace {
    /// The window that will hold it; nil is the background.
    let window: String?
    /// Its place in that window's tabs; nil is the end.
    let position: Int?
    /// Open it in a little window as well (owned by `window`, or by none in the background).
    let little: Bool
    /// Where the little window grows from.
    let card: CGRect?
    /// The pane that moved: the new space is the one holding it.
    let pane: String
    let at: Date
}

/// One space in the "Move Pane to Space" submenu.
struct PaneMoveRow: Equatable {
    let space: String
    let title: String
    /// The pane's own space: shown, marked "here", and not chosen.
    let here: Bool
}

/// The spaces of one window, or the background, in the submenu.
struct PaneMoveGroup: Equatable {
    let title: String
    let rows: [PaneMoveRow]
}

extension AppModel {
    // MARK: The menu

    /// The spaces a pane can move to, grouped under each window's name and then the background.
    func paneMoveGroups(for pane: String) -> [PaneMoveGroup] {
        let home = space(holding: pane)?.id
        func rows(_ spaces: [SpaceInfo]) -> [PaneMoveRow] {
            spaces.map { PaneMoveRow(space: $0.id, title: $0.name, here: $0.id == home) }
        }
        var groups = layout.windows.compactMap { window -> PaneMoveGroup? in
            let held = spaces(in: window.id)
            return held.isEmpty ? nil : PaneMoveGroup(title: title(of: window), rows: rows(held))
        }
        if !backgroundRows.isEmpty { groups.append(PaneMoveGroup(title: "Background", rows: rows(backgroundRows))) }
        return groups
    }

    /// The submenu as the debug state reports it: for every pane, its groups of spaces.
    func paneMoveGroupsJSON() -> [String: Any] {
        let panes = spaces.flatMap { $0.root.panes.map(\.id) }
        return Dictionary(uniqueKeysWithValues: panes.map { pane in
            (pane, paneMoveGroups(for: pane).map { group in
                ["title": group.title, "rows": group.rows.map { ["space": $0.space, "title": $0.title, "here": $0.here] as [String: Any] }] as [String: Any]
            })
        })
    }

    // MARK: Moves

    /// Move `pane` to the end of `space`.
    func movePane(_ pane: String, toEndOf space: String) {
        guard let home = self.space(holding: pane), home.id != space else { return }
        Log.info("pane \(pane): move to the end of space \(space)")
        sendLayoutOp { .paneMove(req: $0, pane: pane, to: .end(space: space)) }
    }

    /// Move `pane` into a new space: in `window` (its own window by default, right after its space; the
    /// background when its space is in the background). With `little`, the new space also opens in a
    /// little window that belongs to that window.
    func movePaneToNewSpace(_ pane: String, in requested: String? = nil, little: Bool = false, card: CGRect? = nil) {
        guard let home = space(holding: pane) else { return }
        let source = layout.window(holding: home.id)
        let window = requested ?? source?.id
        let position = window == source?.id ? source?.spaces.firstIndex(of: home.id).map { $0 + 1 } : nil
        pendingNewSpace = PendingNewSpace(window: window, position: position, little: little, card: card, pane: pane, at: Date())
        Log.info("pane \(pane): move to a new space in \(window.map { "window \($0.prefix(8))" } ?? "the background")\(little ? ", in a little window" : "")")
        sendLayoutOp { .paneMove(req: $0, pane: pane, to: .newSpace(background: window == nil)) }
    }

    // MARK: The drag

    /// What the pointer at `point` (screen coordinates) would do with a pane dragged out of its space:
    /// the popover's rows, a window's tabs or rows, the empty part of its strip, the pane under it
    /// in the space that window shows, or the desktop. nil where nothing happens, which includes the
    /// pane's own space, whose grid the header gesture handles.
    func awayTarget(at point: CGPoint, home: String?) -> (away: PaneAway, hold: (window: String, space: String)?)? {
        if let pop = MenuBarWindow.current, pop.isVisible, pop.frame.contains(point) {
            for rows in popoverRowFrames.values {
                for (space, frame) in rows where space != home && screenRect(frame, in: pop).contains(point) { return (.end(space: space), nil) }
            }
            return nil
        }
        for nswindow in NSApp.orderedWindows where nswindow.isVisible && nswindow.isWheelWindow && nswindow.frame.contains(point) {
            guard let raw = nswindow.identifier?.rawValue, raw.hasPrefix(WindowManager.idPrefix) else { return nil }
            let id = String(raw.dropFirst(WindowManager.idPrefix.count))
            guard let record = layout.window(id) else { return nil }
            for space in record.spaces {
                guard let frame = rowFrames[id]?[space], screenRect(frame, in: nswindow).contains(point) else { continue }
                return space == home ? nil : (.end(space: space), (id, space))
            }
            let strip = record.sidebar ? sidebarFrames[id] : tabStripFrames[id]
            if let strip, screenRect(strip, in: nswindow).contains(point) { return (.newSpace(window: id), nil) }
            if let shown = record.active, shown != home, layout.little(showing: shown) == nil,
               let grid = gridFrames[id].map({ screenRect($0, in: nswindow) }), grid.contains(point),
               let base = baseLayouts[shown],
               let hit = PaneDragging.away(at: CGPoint(x: point.x - grid.minX, y: grid.maxY - point.y), among: base.panes) {
                return (.beside(pane: hit.pane, dir: hit.dir, space: shown), nil)
            }
            return nil
        }
        return (.desktop, nil)
    }

    /// The pointer moved: find where a pane dragged out of its space would land, and follow it with the chip.
    func movePaneDrag(at point: CGPoint? = nil) {
        guard let drag = paneDrag, !drag.committed, !dragCancelled else { return }
        let at = point ?? NSEvent.mouseLocation
        let found = awayTarget(at: at, home: space(holding: drag.pane)?.id)
        setPaneAway(found?.away, hold: found?.hold)
        if paneDrag?.away != nil { showPaneChip(at: at) } else { dragChip?.hide() }
    }

    /// Name where a dragged pane lands outside its space, as the debug endpoint does without a pointer.
    func hoverPaneAway(_ away: PaneAway?) {
        var hold: (window: String, space: String)?
        if case let .end(space)? = away, let window = layout.window(holding: space)?.id { hold = (window, space) }
        setPaneAway(away, hold: hold)
    }

    func setPaneAway(_ away: PaneAway?, hold: (window: String, space: String)? = nil) {
        guard var drag = paneDrag, !drag.committed else { return }
        if drag.away != away {
            drag.away = away
            if away != nil { drag.drop = nil }
            withAnimation(GridMotion.layout) { paneDrag = drag }
            Log.info("pane drag \(drag.pane): over \(away.map { describe($0) } ?? "nothing")")
        }
        armHold(hold)
    }

    /// Holding over a tab for about half a second shows its space, so the pane can be dropped beside one of its panes.
    private func armHold(_ hold: (window: String, space: String)?) {
        guard hold?.space != paneHoldSpace else { return }
        paneHoldTask?.cancel()
        paneHoldTask = nil
        paneHoldSpace = hold?.space
        guard let hold, layout.window(hold.window)?.active != hold.space else { return }
        paneHoldTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(PaneDragging.holdSeconds))
            guard !Task.isCancelled, let self, case .end(hold.space)? = self.paneDrag?.away else { return }
            self.showHeld(hold.space, in: hold.window)
        }
    }

    private func showHeld(_ space: String, in window: String) {
        guard let record = layout.window(window), record.active != space else { return }
        if paneDragShown[window] == nil, let was = record.active { paneDragShown[window] = was }
        Log.info("pane drag: held over space \(space) for \(PaneDragging.holdSeconds) s; window \(window.prefix(8)) shows it")
        editWindows { $0.show(space, in: window) }
    }

    /// The drag ended: stop waiting, drop the chip and, unless the pane landed in a space it opened, put back what each window showed.
    func endAway(landingIn landed: String? = nil) {
        paneHoldTask?.cancel()
        paneHoldTask = nil
        paneHoldSpace = nil
        dragChip?.hide()
        let shown = paneDragShown
        paneDragShown = [:]
        // A window that shows the space the pane landed in keeps it; every other goes back.
        for (window, was) in shown where layout.window(window)?.spaces.contains(was) == true && layout.window(window)?.active != landed {
            editWindows { $0.show(was, in: window) }
        }
    }

    /// Release over `away`: send the move.
    func dropAway(_ pane: String, _ away: PaneAway) {
        Log.info("pane drag \(pane): dropped on \(describe(away))")
        switch away {
        case let .beside(target, dir, space):
            endAway(landingIn: space)
            sendLayoutOp { .paneMove(req: $0, pane: pane, to: .beside(pane: target, dir: dir)) }
        case let .end(space):
            endAway(landingIn: space)
            sendLayoutOp { .paneMove(req: $0, pane: pane, to: .end(space: space)) }
        case let .newSpace(window):
            endAway()
            movePaneToNewSpace(pane, in: window)
        case .desktop:
            endAway()
            let at = NSEvent.mouseLocation
            let size = DesktopDropOverlay.size
            movePaneToNewSpace(pane, little: true, card: CGRect(x: at.x - size.width / 2, y: at.y - size.height / 2, width: size.width, height: size.height))
        }
    }

    func describe(_ away: PaneAway) -> String {
        switch away {
        case let .beside(pane, dir, space): "\(pane) \(dir.rawValue) in space \(space)"
        case let .end(space): "the end of space \(space)"
        case let .newSpace(window): "a new space in window \(window.prefix(8))"
        case .desktop: "the desktop"
        }
    }

    /// What releasing would do, in words: the debug state reports it, no view draws it.
    var paneDragHint: String? {
        guard let drag = paneDrag, let away = drag.away else { return nil }
        return away.hint(pane: drag.pane, spaceName: { self.spaceName($0) }, windowTitle: { id in self.layout.window(id).map { self.title(of: $0) } ?? id })
    }

    /// Whether the dragged pane is over `space`'s tab or row, which draws outlined.
    func paneIsOver(space: String) -> Bool {
        if case let .end(held)? = paneDrag?.away { return held == space }
        return false
    }

    /// Whether the dragged pane is over `window`'s empty tab strip or sidebar, where it would make a new space.
    func paneIsOverNewSpace(in window: String) -> Bool {
        if case let .newSpace(held)? = paneDrag?.away { return held == window }
        return false
    }

    private func showPaneChip(at point: CGPoint) {
        guard let drag = paneDrag else { return }
        if dragChip == nil { dragChip = DragChipWindow() }
        let label = paneLabel(drag.pane)
        dragChip?.show(name: drag.away == .desktop ? "\(label) → new little window" : label, at: point)
    }

    /// A pane's name for the chip that follows the pointer.
    func paneLabel(_ pane: String) -> String {
        switch space(holding: pane)?.root.panes.first(where: { $0.id == pane })?.kind {
        case let .terminal(term, agent)?: agent?.name.capitalized ?? terminals[term]?.view.title.nonEmpty ?? "Terminal"
        case let .agent(_, agent)?: agent.capitalized
        case let .web(artifact)?: artifact.name
        case .browser?: "Browser"
        case .empty?: "Empty pane"
        default: pane
        }
    }
}

private extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}

/// The accent outline a tab, row or strip draws while a dragged pane is over it.
struct PaneDropOutline: View {
    let cornerRadius: CGFloat

    var body: some View {
        RoundedRectangle(cornerRadius: cornerRadius)
            .fill(Color.accentColor.opacity(0.14))
            .overlay(RoundedRectangle(cornerRadius: cornerRadius).strokeBorder(Color.accentColor, lineWidth: 2))
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

/// "Move Pane to Space ▸" in a pane header's menu: the spaces under each window's name, then the background.
struct MovePaneMenu: View {
    @ObservedObject var model: AppModel
    let pane: String

    var body: some View {
        Menu("Move Pane to Space") {
            ForEach(model.paneMoveGroups(for: pane), id: \.title) { group in
                Section(group.title) {
                    ForEach(group.rows, id: \.space) { row in
                        Button(row.here ? "\(row.title) — here" : row.title) { model.movePane(pane, toEndOf: row.space) }
                            .disabled(row.here)
                    }
                }
            }
        }
    }
}
