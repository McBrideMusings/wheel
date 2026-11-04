import SwiftUI

// MARK: - The controls a chat draws

/// Every clickable control in a chat, with where it sits in the window and whether its hover state
/// is on. The controls report here as they appear; the debug server reads it to say which control a
/// window point is over, and can hold a pointer at a point so the hover state a real pointer
/// causes can be checked without one.
@MainActor
public final class ChatControlRegistry: ObservableObject {
    public static let shared = ChatControlRegistry()

    public struct Entry {
        /// `copy`, `code-copy`, `disclosure`, `edits-toggle`, `queue`, `composer`, `image`, `link`, …
        public var kind: String
        /// What a click does.
        public var label: String
        /// In window coordinates, top left origin; empty until `tracking` is on.
        public var frame: CGRect = .zero
        public var hover = false
        public var focus = false
        public var url: String?
    }

    /// Whether controls record their frames. Off in a normal run, because a frame changes on every
    /// scrolled pixel; the debug server turns it on.
    public var tracking = false

    /// A pointer held at a window point by the debug server, standing in for the real one.
    @Published public private(set) var probe: CGPoint?

    private var controls: [UUID: Entry] = [:]
    private var linkProviders: [String: () -> [Entry]] = [:]
    private var linkHover: [String: Int] = [:]

    public func hold(at point: CGPoint?) { probe = point }

    func add(_ id: UUID, kind: String, label: String) {
        controls[id] = Entry(kind: kind, label: label)
    }

    func remove(_ id: UUID) { controls[id] = nil }

    func move(_ id: UUID, frame: CGRect) {
        guard tracking else { return }
        controls[id]?.frame = frame
    }

    func set(_ id: UUID, hover: Bool? = nil, focus: Bool? = nil) {
        if let hover { controls[id]?.hover = hover }
        if let focus { controls[id]?.focus = focus }
    }

    /// The control that owns `point` when several contain it: the smallest.
    func top(at point: CGPoint) -> UUID? {
        controls.filter { $0.value.frame.contains(point) }
            .min { $0.value.frame.width * $0.value.frame.height < $1.value.frame.width * $1.value.frame.height }?.key
    }

    func setLinks(_ key: String, _ provider: @escaping () -> [Entry]) { linkProviders[key] = provider }

    func removeLinks(_ key: String) {
        linkProviders[key] = nil
        linkHover[key] = nil
    }

    func setLinkHover(_ key: String, _ index: Int?) { linkHover[key] = index }

    func linkHovered(_ key: String) -> Int? { linkHover[key] }

    /// Every control on screen, links included.
    public var all: [Entry] {
        Array(controls.values) + linkProviders.values.flatMap { $0() }
    }

    /// The control under a window point.
    public func control(at point: CGPoint) -> Entry? {
        all.filter { $0.frame.contains(point) }
            .min { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }
    }
}

// MARK: - One treatment for all of them

/// What every clickable thing in a chat does under a pointer: a rounded fill while the pointer is
/// over it or the keyboard is on it, a stronger one while it is pressed, and the pointing hand.
struct ChatControlSurface<Content: View>: View {
    let kind: String
    let label: String
    var pressed = false
    var focused = false
    @ViewBuilder let content: Content

    @ObservedObject private var registry = ChatControlRegistry.shared
    @Environment(\.isEnabled) private var enabled
    @State private var id = UUID()
    @State private var pointer = false

    private static var inset: (x: CGFloat, y: CGFloat) { (6, 3) }

    var body: some View {
        let probed = registry.probe.map { registry.top(at: $0) == id } ?? false
        let hover = enabled && (pointer || probed)
        let fill = pressed ? 0.16 : hover || focused ? 0.10 : 0
        let (x, y) = Self.inset
        content
            .padding(.horizontal, x)
            .padding(.vertical, y)
            .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color.primary.opacity(fill)))
            .contentShape(Rectangle())
            .onHover { pointer = $0 }
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { registry.move(id, frame: $0) }
            #if os(macOS)
            .pointerStyle(enabled ? .link : nil)
            #endif
            .padding(.horizontal, -x)
            .padding(.vertical, -y)
            .onAppear {
                registry.add(id, kind: kind, label: label)
                registry.set(id, hover: hover, focus: focused)
            }
            .onDisappear { registry.remove(id) }
            .onChange(of: hover) { registry.set(id, hover: hover) }
            .onChange(of: focused) { registry.set(id, focus: focused) }
    }
}

/// The button style of every button in a chat. `kind` names the kind of control and `label` what a
/// click does; both show up in the debug state.
public struct ChatControlStyle: ButtonStyle {
    let kind: String
    let label: String

    public func makeBody(configuration: Configuration) -> some View {
        Styled(kind: kind, label: label, configuration: configuration)
    }

    private struct Styled: View {
        let kind: String
        let label: String
        let configuration: ButtonStyleConfiguration
        @Environment(\.isFocused) private var focused

        var body: some View {
            ChatControlSurface(kind: kind, label: label, pressed: configuration.isPressed, focused: focused) {
                configuration.label
            }
        }
    }
}

extension ButtonStyle where Self == ChatControlStyle {
    /// A chat's button: `kind` names it, `label` says what a click does.
    public static func chat(_ kind: String, _ label: String) -> ChatControlStyle {
        ChatControlStyle(kind: kind, label: label)
    }
}

extension View {
    /// The same treatment for a control that is not a `Button`: a tap gesture, a menu.
    public func chatControl(_ kind: String, _ label: String) -> some View {
        ChatControlSurface(kind: kind, label: label) { self }
    }
}

// MARK: - Links in a reply

/// A reply's links under the pointer: a rounded wash behind the link's text and the pointing hand.
/// Textual draws the links and takes their clicks but does not say where a link sits, so the
/// ranges come from the reply's rendered text and the rectangles from the layout the selection
/// already reads.
struct ChatLinkHover: ViewModifier {
    let prose: RenderedProse
    let id: Int
    let chat: SessionChat

    @ObservedObject private var registry = ChatControlRegistry.shared
    @State private var pointer: Int?
    @State private var probed: Int?

    private var key: String { "\(chat.name)/\(id)" }
    private var active: Int? { pointer ?? probed }

    /// The link under a point in the reply's own coordinates.
    private func link(at point: CGPoint) -> Int? {
        guard let drawn = chat.selection.drawn[id] else { return nil }
        return prose.links.indices.first { index in
            drawn.rects(for: prose.links[index].range).contains { $0.insetBy(dx: -1, dy: -1).contains(point) }
        }
    }

    private func entries() -> [ChatControlRegistry.Entry] {
        guard let drawn = chat.selection.drawn[id], let origin = chat.selection.targets[id]?.frame.origin else { return [] }
        let text = prose.text as NSString
        let hovered = registry.linkHovered(key)
        return prose.links.enumerated().compactMap { index, link in
            let rects = drawn.rects(for: link.range)
            guard let first = rects.first else { return nil }
            let frame = rects.dropFirst().reduce(first) { $0.union($1) }.offsetBy(dx: origin.x, dy: origin.y)
            let label = text.substring(with: NSRange(location: link.range.lowerBound, length: link.range.count))
            return ChatControlRegistry.Entry(kind: "link", label: label, frame: frame, hover: hovered == index, url: link.url)
        }
    }

    func body(content: Content) -> some View {
        content
            .overlay(alignment: .topLeading) {
                if let active, prose.links.indices.contains(active), let drawn = chat.selection.drawn[id] {
                    ZStack(alignment: .topLeading) {
                        ForEach(Array(drawn.rects(for: prose.links[active].range).enumerated()), id: \.offset) { _, rect in
                            RoundedRectangle(cornerRadius: 4, style: .continuous)
                                .fill(Color.primary.opacity(0.12))
                                .frame(width: rect.width + 4, height: rect.height)
                                .offset(x: rect.minX - 2, y: rect.minY)
                        }
                    }
                    .allowsHitTesting(false)
                }
            }
            #if os(macOS)
            .pointerStyle(active != nil ? .link : nil)
            #endif
            .onContinuousHover { phase in
                let now: Int?
                switch phase {
                case let .active(point): now = link(at: point)
                case .ended: now = nil
                }
                if now != pointer { pointer = now }
            }
            .onChange(of: registry.probe) { _, point in
                guard let point, let origin = chat.selection.targets[id]?.frame.origin else { probed = nil; return }
                probed = link(at: CGPoint(x: point.x - origin.x, y: point.y - origin.y))
            }
            .onChange(of: active) { registry.setLinkHover(key, active) }
            .onAppear { registry.setLinks(key, entries) }
            .onChange(of: prose) { registry.setLinks(key, entries) }
            .onDisappear { registry.removeLinks(key) }
    }
}

extension View {
    func chatLinkHover(_ prose: RenderedProse, id: Int, chat: SessionChat) -> some View {
        modifier(ChatLinkHover(prose: prose, id: id, chat: chat))
    }
}
