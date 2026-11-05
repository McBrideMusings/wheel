import SwiftUI
import WheelKit
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// A pane's or space's status as a mark: a 9pt dot (orange needs you, a hollow red ring stopped, green idle) or,
/// while a turn runs, a 12pt blue arc turning once a second. It draws nothing for no status or a
/// rank `status.dots` leaves out.
public struct StatusDot: View {
    let status: PaneStatus?
    let shown: Set<StatusRank>

    public init(status: PaneStatus?, shown: Set<StatusRank>) {
        self.status = status
        self.shown = shown
    }

    public var body: some View {
        if let status, shown.contains(status.rank) {
            mark(status)
                .help(status.label)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(status.label)
        }
    }

    @ViewBuilder
    private func mark(_ status: PaneStatus) -> some View {
        switch status {
        case .working: WorkingArc()
        case .needsYou: Circle().fill(Color.orange).frame(width: 9, height: 9)
        case .stopped: Circle().strokeBorder(Color.red, lineWidth: 1.8).frame(width: 9, height: 9)
        case .idle: Circle().fill(Color.green).frame(width: 9, height: 9)
        }
    }
}

/// A space's agents as marks: one per agent pane, most urgent first, overlapping a little, at most
/// `SpaceInfo.agentDotLimit` and then `+N`. A space whose panes sent no statuses but which has one of
/// its own (a daemon that predates per-pane statuses) draws that one.
public struct SpaceDots: View {
    let space: SpaceInfo
    let shown: Set<StatusRank>

    public init(space: SpaceInfo, shown: Set<StatusRank>) {
        self.space = space
        self.shown = shown
    }

    public var body: some View {
        let agents = space.agentStatuses(shown: shown)
        if space.paneStatuses.isEmpty {
            StatusDot(status: space.status, shown: shown)
        } else if !agents.marks.isEmpty {
            HStack(spacing: 3) {
                // Each mark gets the arc's 12pt slot, so a dot sits centred in it and two arcs
                // touch rather than merge; the slots overlap by 2pt.
                HStack(spacing: -2) {
                    ForEach(Array(agents.marks.enumerated()), id: \.offset) { index, status in
                        StatusDot(status: status, shown: shown)
                            .frame(width: 12, height: 12)
                            .zIndex(Double(agents.marks.count - index))
                    }
                }
                if agents.more > 0 {
                    Text("+\(agents.more)")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Self.label(agents))
        }
    }

    /// What VoiceOver reads: "Needs you, Working, 2 more".
    static func label(_ agents: (marks: [PaneStatus], more: Int)) -> String {
        var parts = agents.marks.map { $0.label }
        if agents.more > 0 { parts.append("\(agents.more) more") }
        return parts.joined(separator: ", ")
    }
}

/// The 3/4 arc, turning clockwise once a second. Its own view so each time a pane starts working
/// the arc starts from rest, and it stands still under Reduce Motion. The turn is a Core Animation
/// animation on the arc's layer, run by the render server: a SwiftUI animation would step on the
/// main thread and redraw the whole window every frame for as long as any agent works.
private struct WorkingArc: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ArcLayer(turning: !reduceMotion)
            .frame(width: 12, height: 12)
    }
}

#if os(macOS)
private struct ArcLayer: NSViewRepresentable {
    let turning: Bool
    func makeNSView(context: Context) -> ArcView { ArcView() }
    func updateNSView(_ view: ArcView, context: Context) { view.turning = turning }
}

private final class ArcView: NSView {
    private let arc = CAShapeLayer()
    var turning = false { didSet { if turning != oldValue { spin() } } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        arc.fillColor = nil
        arc.lineWidth = 1.8
        arc.lineCap = .round
        arc.strokeEnd = 0.75
        layer?.addSublayer(arc)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        arc.frame = bounds
        arc.path = CGPath(ellipseIn: bounds, transform: nil)
        effectiveAppearance.performAsCurrentDrawingAppearance { arc.strokeColor = NSColor.systemBlue.cgColor }
        CATransaction.commit()
    }

    override func viewDidChangeEffectiveAppearance() { needsLayout = true }

    /// A mark, not a control: a click goes to the row or tab under it.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// A layer moved into another window may come back without its animations; start the turn again.
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil { spin() }
    }

    /// AppKit's y axis points up, so a clockwise turn is a negative angle.
    private func spin() {
        arc.removeAnimation(forKey: "turn")
        guard turning else { return }
        arc.add(turnAnimation(to: -2 * .pi), forKey: "turn")
    }
}
#else
private struct ArcLayer: UIViewRepresentable {
    let turning: Bool
    func makeUIView(context: Context) -> ArcView { ArcView() }
    func updateUIView(_ view: ArcView, context: Context) { view.turning = turning }
}

private final class ArcView: UIView {
    private let arc = CAShapeLayer()
    var turning = false { didSet { if turning != oldValue { spin() } } }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        arc.fillColor = nil
        arc.lineWidth = 1.8
        arc.lineCap = .round
        arc.strokeEnd = 0.75
        layer.addSublayer(arc)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func layoutSubviews() {
        super.layoutSubviews()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        arc.frame = bounds
        arc.path = CGPath(ellipseIn: bounds, transform: nil)
        arc.strokeColor = UIColor.systemBlue.resolvedColor(with: traitCollection).cgColor
        CATransaction.commit()
    }

    /// A window move drops a layer's animations on iOS; start the turn again in the new one.
    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        setNeedsLayout()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil { spin() }
    }

    private func spin() {
        arc.removeAnimation(forKey: "turn")
        guard turning else { return }
        arc.add(turnAnimation(to: 2 * .pi), forKey: "turn")
    }
}
#endif

/// One full turn a second, forever, that survives the layer leaving and rejoining a window.
private func turnAnimation(to angle: CGFloat) -> CABasicAnimation {
    let turn = CABasicAnimation(keyPath: "transform.rotation.z")
    turn.fromValue = 0
    turn.toValue = angle
    turn.duration = 1
    turn.repeatCount = .infinity
    turn.isRemovedOnCompletion = false
    return turn
}
