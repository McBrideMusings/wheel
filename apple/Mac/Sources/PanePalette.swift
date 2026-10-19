import Foundation
import WheelKit

/// The artifact palette open inside one pane. The pure state (listing, query, selection) is
/// WheelKit's `ArtifactPalette`; this adds what only the window has.
@MainActor
final class PanePalette: ObservableObject {
    /// The kind the pane had when the palette opened (`PaneKind.name`).
    let openedOver: String
    @Published var state = ArtifactPalette()
    /// A choice waiting on the person's yes because it would end the pane's shell or agent.
    @Published var confirming: ArtifactInfo?
    /// Whether the search field holds the keyboard.
    var fieldFocused = false

    init(openedOver: String) {
        self.openedOver = openedOver
    }
}

/// The palette's operations: the pane menus, the palette's own keys and the debug endpoint all
/// call these.
extension AppModel {
    /// "Artifact…": open the palette inside `pane` and list the artifacts its space offers.
    func openPalette(_ pane: String) {
        guard palettes[pane] == nil, let space = spaces.first(where: { $0.root.panes.contains { $0.id == pane } }),
              let kind = space.root.panes.first(where: { $0.id == pane })?.kind else { return }
        Log.info("palette \(pane): opened (space \(space.id))")
        palettes[pane] = PanePalette(openedOver: kind.name)
        focus(pane: pane)
        askArtifacts(pane)
    }

    /// List again for `pane`'s palette: the `pane = true` artifacts of the space the pane is in.
    func askArtifacts(_ pane: String) {
        guard let palette = palettes[pane] else { return }
        palette.state.relisting()
        guard connection == .connected, let space = spaces.first(where: { $0.root.panes.contains { $0.id == pane } }) else {
            palette.state.failed("wheeld is not connected.")
            return
        }
        // An earlier listing for this pane that is still out is no longer wanted.
        paletteReqs = paletteReqs.filter { $0.value != pane }
        let req = client.req()
        paletteReqs[req] = pane
        Log.info("palette \(pane): artifact ls for space \(space.id)")
        client.send(.artifactLs(req: req, space: space.id, paneOnly: true))
    }

    func closePalette(_ pane: String) {
        guard palettes[pane] != nil else { return }
        Log.info("palette \(pane): closed")
        palettes[pane] = nil
        paletteReqs = paletteReqs.filter { $0.value != pane }
    }

    func paletteTyped(_ pane: String, _ text: String) {
        guard let palette = palettes[pane], palette.state.searchable, palette.state.query != text else { return }
        palette.state.type(text)
        Log.info("palette \(pane): query \"\(text)\" -> \(palette.state.rows.count) of \(palette.state.all.count)")
    }

    func paletteMoved(_ pane: String, by step: Int) {
        guard let palette = palettes[pane] else { return }
        palette.state.move(step)
    }

    /// Esc: the query first, then the palette.
    func paletteEscaped(_ pane: String) {
        guard let palette = palettes[pane] else { return }
        if palette.confirming != nil {
            palette.confirming = nil
        } else if palette.state.escape() {
            closePalette(pane)
        }
    }

    /// Return: open the selected row, or list again after a failure.
    func paletteEntered(_ pane: String) {
        guard let palette = palettes[pane] else { return }
        if palette.state.failure != nil {
            askArtifacts(pane)
        } else if let row = palette.state.selectedRow {
            paletteChose(pane, row.info)
        }
    }

    /// A row, chosen by Return or a click. Showing an artifact in a pane that runs a shell or an
    /// agent ends it, so that asks first.
    func paletteChose(_ pane: String, _ info: ArtifactInfo) {
        guard let palette = palettes[pane] else { return }
        palette.state.select(info.id)
        let kind = spaces.lazy.flatMap { $0.root.panes }.first { $0.id == pane }?.kind
        if kind?.endsASession == true {
            Log.info("palette \(pane): \(info.id) chosen; asking before it ends the pane's session")
            palette.confirming = info
        } else {
            showArtifact(pane, info)
        }
    }

    /// Send the choice: the artifact's scope and name, never a path.
    func showArtifact(_ pane: String, _ info: ArtifactInfo) {
        guard connection == .connected else {
            palettes[pane]?.state.failed("wheeld is not connected.")
            return
        }
        Log.info("palette \(pane): opening \(info.id)")
        closePalette(pane)
        setKind(pane, .web, artifact: info.name, scope: info.scope)
    }
}
