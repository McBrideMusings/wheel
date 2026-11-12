import AppKit

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
