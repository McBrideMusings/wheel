import SwiftUI
import WheelKit

/// Where this client pins the tray chip along its window's edges (ADR-0022: the position is the
/// client's own, never sent to wheeld). `along` is 0...1 from the edge's start.
struct TrayPlace: Equatable {
    var edge: Dir = .right
    var along: CGFloat = 0.5

    static let key = "wheel.trayChipPlace"

    static func load() -> TrayPlace {
        let parts = (UserDefaults.standard.string(forKey: key) ?? "").split(separator: ":")
        guard parts.count == 2, let edge = Dir(rawValue: String(parts[0])), let along = Double(parts[1]) else { return TrayPlace() }
        return TrayPlace(edge: edge, along: min(max(CGFloat(along), 0), 1))
    }

    func save() { UserDefaults.standard.set("\(edge.rawValue):\(Double(along))", forKey: Self.key) }

    static let size: CGFloat = 34
    static let margin: CGFloat = 8

    /// The chip's top-left corner in a grid of `bounds`.
    func origin(in bounds: CGSize) -> CGPoint {
        let side = Self.size, m = Self.margin
        let x = max(bounds.width - side - 2 * m, 0) * along + m
        let y = max(bounds.height - side - 2 * m, 0) * along + m
        switch edge {
        case .left: return CGPoint(x: m, y: y)
        case .right: return CGPoint(x: bounds.width - side - m, y: y)
        case .up: return CGPoint(x: x, y: m)
        case .down: return CGPoint(x: x, y: bounds.height - side - m)
        }
    }

    /// The nearest edge to `point` and where along it the point falls.
    static func nearest(to point: CGPoint, in bounds: CGSize) -> TrayPlace {
        let gaps: [(Dir, CGFloat)] = [(.left, point.x), (.right, bounds.width - point.x), (.up, point.y), (.down, bounds.height - point.y)]
        let edge = gaps.min { $0.1 < $1.1 }?.0 ?? .right
        let span = (edge == .left || edge == .right ? bounds.height : bounds.width) - size - 2 * margin
        let pos = (edge == .left || edge == .right ? point.y : point.x) - size / 2 - margin
        return TrayPlace(edge: edge, along: span > 0 ? min(max(pos / span, 0), 1) : 0.5)
    }
}

/// One stacked-panes chip, present only while the space has backgrounded panes. An orange badge
/// dot shows only while one of them needs the person; a click lists them.
struct TrayChip: View {
    var model: AppModel
    let space: SpaceInfo
    let bounds: CGSize
    @State private var dragTo: CGPoint?
    private var place: TrayPlace { model.trayPlace }

    private var panes: [(id: String, kind: PaneKind)] { space.backgrounded.flatMap(\.panes).map { ($0.id, $0.kind) } }
    private var needsYou: Bool {
        panes.contains { if case .needsYou? = model.paneStatus($0.id) { true } else { false } }
    }

    var body: some View {
        let origin = dragTo.map { CGPoint(x: $0.x - TrayPlace.size / 2, y: $0.y - TrayPlace.size / 2) } ?? place.origin(in: bounds)
        Image(systemName: "square.stack")
            .font(.system(size: 15, weight: .medium))
            .foregroundStyle(.primary)
            .frame(width: TrayPlace.size, height: TrayPlace.size)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 9))
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(.separator))
            .overlay(alignment: .topTrailing) {
                if needsYou { Circle().fill(Color.orange).frame(width: 10, height: 10).offset(x: 3, y: -3) }
            }
            .shadow(color: .black.opacity(0.25), radius: 5, y: 2)
            .contentShape(RoundedRectangle(cornerRadius: 9))
            .onTapGesture { model.setTrayOpen(space.id, !model.trayOpen(space.id)) }
            .gesture(
                DragGesture(minimumDistance: 6, coordinateSpace: .named("spaceGrid"))
                    .onChanged { dragTo = $0.location }
                    .onEnded {
                        model.trayPlace = TrayPlace.nearest(to: $0.location, in: bounds)
                        dragTo = nil
                    }
            )
            .popover(isPresented: Binding(get: { model.trayOpen(space.id) }, set: { model.setTrayOpen(space.id, $0) }),
                     arrowEdge: place.edge == .left ? .trailing : place.edge == .right ? .leading : place.edge == .up ? .bottom : .top) {
                TrayList(model: model, space: space, panes: panes)
            }
            .help("Backgrounded panes")
            .accessibilityLabel(needsYou ? "Backgrounded panes, one needs you" : "Backgrounded panes")
            .accessibilityIdentifier("tray chip")
            .offset(x: origin.x, y: origin.y)
            .onChange(of: place.origin(in: bounds), initial: true) { _, new in
                model.trayChipFrames[space.id] = CGRect(origin: new, size: CGSize(width: TrayPlace.size, height: TrayPlace.size))
            }
            .onDisappear {
                model.trayChipFrames[space.id] = nil
                model.setTrayOpen(space.id, false)
            }
            .zIndex(4)
    }
}

/// The popover list: one row per backgrounded pane with its own icon and title and no status dot.
/// A row returns the pane to the grid; its pop-out icon pops it out.
private struct TrayList: View {
    var model: AppModel
    let space: SpaceInfo
    let panes: [(id: String, kind: PaneKind)]

    var body: some View {
        VStack(spacing: 0) {
            ForEach(panes, id: \.id) { pane in
                HStack(spacing: 8) {
                    Button {
                        model.setTrayOpen(space.id, false)
                        model.setSite(pane.id, .grid)
                    } label: {
                        HStack(spacing: 8) {
                            Image(systemName: pane.kind.icon).foregroundStyle(.secondary).frame(width: 18)
                            Text(model.trayTitle(pane.kind)).lineLimit(1)
                            Spacer(minLength: 12)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("tray row \(pane.id)")
                    Button {
                        model.setTrayOpen(space.id, false)
                        model.setSite(pane.id, .popped)
                    } label: { Image(systemName: "rectangle.on.rectangle") }
                        .buttonStyle(.borderless)
                        .help("Pop out")
                        .accessibilityLabel("Pop out")
                        .accessibilityIdentifier("tray pop \(pane.id)")
                }
                .padding(.horizontal, 12)
                .frame(height: 30)
            }
        }
        .padding(.vertical, 6)
        .frame(minWidth: 220)
    }
}
