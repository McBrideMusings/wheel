import AppKit
import SwiftUI
import WheelKit

/// One of the icon buttons a popover row shows, and whether it names where the space is now.
struct PlaceButton: Equatable, Identifiable {
    let id: String
    let help: String
    let lit: Bool
}

extension AppModel {
    /// A window's number: its place in the order the popover lists the windows, from 1.
    func windowNumber(_ id: String) -> Int? { layout.windows.firstIndex { $0.id == id }.map { $0 + 1 } }

    /// The spaces in a little window that belongs to no main window.
    var ownerlessLittleSpaces: [SpaceInfo] {
        layout.littles.filter { $0.owner == nil }.compactMap { little in spaces.first { $0.id == little.space } }
    }

    /// The spaces that run in a session no window shows, then those in an ownerless little window:
    /// what the status item's badge counts.
    var windowlessSpaces: [SpaceInfo] { backgroundSpaces + ownerlessLittleSpaces }

    /// The buttons a row shows: Little Window. Where a space lives is its session's (the row's menu moves it).
    func placeButtons(for space: String) -> [PlaceButton] {
        let little = layout.little(showing: space)
        return [PlaceButton(id: "little", help: little == nil ? "Open in a little window" : "Go to its little window", lit: little != nil)]
    }

    /// Press a row's button by id, as a click on it does. Returns false when the row has no such button.
    @discardableResult
    func runPlace(_ id: String, space: String) -> Bool {
        guard spaces.contains(where: { $0.id == space }),
              let button = placeButtons(for: space).first(where: { $0.id == id }) else { return false }
        Log.info("popover place \(space): \(id)\(button.lit ? " (already there)" : "")")
        openLittle(for: space)
        return true
    }
}

/// The small drawn marks the popover and the tab strip share, on a 16-point grid: a window with its
/// number and a little window.
struct PlaceGlyph: View {
    enum Kind: Equatable {
        case window(Int)
        case little
    }

    let kind: Kind
    var size: CGFloat = 16

    var body: some View {
        Canvas { context, canvas in
            let u = canvas.width / 16
            let line = 1.2 * u
            func frame() -> Path { Path(roundedRect: CGRect(x: 1.5 * u, y: 2.5 * u, width: 13 * u, height: 11 * u), cornerRadius: 2 * u) }
            func bar() -> Path {
                var path = Path()
                path.move(to: CGPoint(x: 1.5 * u, y: 5.5 * u))
                path.addLine(to: CGPoint(x: 14.5 * u, y: 5.5 * u))
                return path
            }
            switch kind {
            case let .window(number):
                context.stroke(frame(), with: .foreground, lineWidth: line)
                context.stroke(bar(), with: .foreground, lineWidth: line)
                context.draw(Text("\(number)").font(.system(size: 6.5 * u, weight: .bold)), at: CGPoint(x: 8 * u, y: 9.6 * u))
            case .little:
                context.stroke(frame(), with: .foreground, lineWidth: line)
                context.fill(Path(roundedRect: CGRect(x: 8 * u, y: 8 * u, width: 5 * u, height: 4 * u), cornerRadius: 1 * u), with: .foreground)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// A row's place buttons, shown while the pointer is over the row.
struct PlaceButtons: View {
    var model: AppModel
    let space: SpaceInfo

    var body: some View {
        HStack(spacing: 0) {
            ForEach(model.placeButtons(for: space.id)) { button in
                Button { model.runPlace(button.id, space: space.id) } label: {
                    PlaceGlyph(kind: .little).frame(width: 22, height: 20).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(button.lit ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
                .help(button.help)
                .accessibilityLabel(button.help)
                .accessibilityAddTraits(button.lit ? .isSelected : [])
            }
        }
    }

}

/// The menu bar item's image: the Wheel mark, with the number of spaces in windowless sessions in a badge at
/// its top right. A template image, so the system draws it in the menu bar's own colour; the badge
/// is a capsule with the number cut out of it.
enum MenuBarIcon {
    /// `update` adds a dot at the lower right: a newer server is installed and waits for a restart.
    static func image(background count: Int, update: Bool = false) -> NSImage {
        let configuration = NSImage.SymbolConfiguration(pointSize: 14, weight: .regular)
        let mark = NSImage(systemSymbolName: "steeringwheel", accessibilityDescription: nil)?.withSymbolConfiguration(configuration)
        let markSize = mark?.size ?? NSSize(width: 16, height: 16)
        let text = count > 9 ? "9+" : "\(count)"
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 8, weight: .bold)]
        let label = NSAttributedString(string: text, attributes: attributes)
        let badgeHeight: CGFloat = 11
        let badgeWidth = count > 0 ? max(badgeHeight, ceil(label.size().width) + 5) : 0
        let dot: CGFloat = 7
        let extra = max(count > 0 ? badgeWidth * 0.45 : 0, update ? dot * 0.45 : 0)
        let size = NSSize(width: ceil(markSize.width) + extra, height: 18)
        let image = NSImage(size: size, flipped: false) { rect in
            mark?.draw(in: NSRect(x: 0, y: (rect.height - markSize.height) / 2, width: markSize.width, height: markSize.height))
            guard let context = NSGraphicsContext.current else { return true }
            if update {
                let spot = NSRect(x: rect.width - dot, y: 0, width: dot, height: dot)
                context.compositingOperation = .clear
                NSBezierPath(ovalIn: spot.insetBy(dx: -1.5, dy: -1.5)).fill()
                context.compositingOperation = .sourceOver
                NSColor.black.setFill()
                NSBezierPath(ovalIn: spot).fill()
            }
            guard count > 0 else { return true }
            let badge = NSRect(x: rect.width - badgeWidth, y: rect.height - badgeHeight, width: badgeWidth, height: badgeHeight)
            // A gap around the badge keeps it apart from the mark.
            context.compositingOperation = .clear
            NSBezierPath(roundedRect: badge.insetBy(dx: -1.5, dy: -1.5), xRadius: badgeHeight / 2 + 1.5, yRadius: badgeHeight / 2 + 1.5).fill()
            context.compositingOperation = .sourceOver
            NSColor.black.setFill()
            NSBezierPath(roundedRect: badge, xRadius: badgeHeight / 2, yRadius: badgeHeight / 2).fill()
            context.compositingOperation = .clear
            let textSize = label.size()
            label.draw(at: NSPoint(x: badge.midX - textSize.width / 2, y: badge.midY - textSize.height / 2))
            context.compositingOperation = .sourceOver
            return true
        }
        image.isTemplate = true
        return image
    }
}
