import SwiftUI
import WheelKit
import WheelUI

/// Orchestrator mode's floating button: a 52 pt circle holding the mode's glyph, ringed by the orchestrator's state, a
/// badge with the count when requests wait on the person, a turning arc while it works, a plain ring when idle. A tap
/// returns to the chat. A drag moves it, and on release it snaps to the nearer side at the height it was let go, clear
/// of the top and bottom bars; the side and height are saved.
struct OrchestratorFloat: View {
    @ObservedObject var chat: AgentChat
    /// The requests the orchestrator waits on the person for.
    let waiting: Int
    /// How far above the area's bottom edge the button stays: the bottom bar's height.
    let bottomClear: CGFloat
    let back: () -> Void
    @AppStorage("orchestratorFloat.leading") private var leading = false
    /// The button's centre as a share of the area's height, nil until the person moves it.
    @AppStorage("orchestratorFloat.height") private var height = -1.0
    @State private var drag: CGSize = .zero
    @State private var turning = false

    static let size: CGFloat = 52
    private static let margin: CGFloat = 16

    var body: some View {
        GeometryReader { geo in
            let centre = centre(in: geo.size)
            button
                .position(x: centre.x + drag.width, y: centre.y + drag.height)
                .gesture(DragGesture(minimumDistance: 8, coordinateSpace: .local)
                    .onChanged { drag = $0.translation }
                    .onEnded { end in
                        let x = centre.x + end.predictedEndTranslation.width
                        let y = clamp(centre.y + end.translation.height, in: geo.size)
                        withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                            leading = x < geo.size.width / 2
                            height = Double(y / max(geo.size.height, 1))
                            drag = .zero
                        }
                        Log.info("float: dragged to the \(leading ? "left" : "right") at \(Int(y)) pt")
                    })
        }
    }

    private var state: String { waiting > 0 ? "needs you" : chat.isBusy ? "working" : "idle" }

    private var button: some View {
        Button(action: back) {
            ZStack {
                Circle().fill(.regularMaterial)
                Circle().strokeBorder(Color.secondary.opacity(0.35), lineWidth: 2)
                if chat.isBusy && waiting == 0 {
                    Circle()
                        .trim(from: 0, to: 0.28)
                        .stroke(Color.accentColor, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                        .padding(1.25)
                        .rotationEffect(.degrees(turning ? 360 : 0))
                        .animation(.linear(duration: 1.2).repeatForever(autoreverses: false), value: turning)
                        .onAppear { turning = true }
                        .onDisappear { turning = false }
                }
                Image(systemName: WindowMode.entry(WindowMode.orchestrator).symbol)
                    .font(.system(size: 20, weight: .medium))
                    .foregroundStyle(.primary)
            }
            .frame(width: Self.size, height: Self.size)
            .overlay(alignment: .topTrailing) {
                if waiting > 0 {
                    Text("\(waiting)")
                        .font(.caption2.weight(.bold).monospacedDigit())
                        .foregroundStyle(.white)
                        .frame(minWidth: 18, minHeight: 18)
                        .background(Color.orange, in: Capsule())
                        .offset(x: 2, y: -2)
                }
            }
            .shadow(color: .black.opacity(0.18), radius: 6, y: 2)
            .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Global orchestrator")
        .accessibilityValue(state)
        .accessibilityIdentifier("orchestrator float")
        .onChange(of: state, initial: true) { _, state in Log.info("float: \(state)") }
    }

    private func centre(in size: CGSize) -> CGPoint {
        let x = leading ? Self.margin + Self.size / 2 : size.width - Self.margin - Self.size / 2
        let y = height < 0 ? size.height - bottomClear - Self.margin - Self.size / 2 : CGFloat(height) * size.height
        return CGPoint(x: x, y: clamp(y, in: size))
    }

    /// Kept clear of the top edge (the top bar is above the area) and of the bottom bar.
    private func clamp(_ y: CGFloat, in size: CGSize) -> CGFloat {
        let low = Self.margin + Self.size / 2
        let high = size.height - bottomClear - Self.margin - Self.size / 2
        return min(max(y, low), max(high, low))
    }
}
