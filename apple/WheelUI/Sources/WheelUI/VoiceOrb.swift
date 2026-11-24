#if os(iOS)
import SwiftUI

/// Voice mode's orb (`docs/voice-mode/index.md` Orb states): blue with rings that follow the microphone
/// while listening, gray and slowly breathing while thinking, bright and moving with the speech while
/// speaking, dimmed while muted. The rings, at their widest, fill `size` x `size`, so the orb never moves
/// or overlaps what is around it.
public struct VoiceOrb: View {
    let face: VoiceFace
    /// `0...1`: the microphone's level while listening, the speech's while speaking.
    let level: Float
    var size: CGFloat = 140

    public init(face: VoiceFace, level: Float, size: CGFloat = 140) {
        self.face = face
        self.level = level
        self.size = size
    }

    private var tint: Color {
        switch face {
        case .listening: .blue
        case .thinking: .gray
        case .speaking: .cyan
        case .muted: .gray
        }
    }

    public var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: face != .thinking)) { timeline in
            let t = timeline.date.timeIntervalSinceReferenceDate
            let breath = face == .thinking ? 1 + 0.05 * sin(t * 1.4) : 1
            let move = CGFloat(face == .muted || face == .thinking ? 0 : level)
            ZStack {
                if face == .listening || face == .speaking {
                    ForEach(1..<3, id: \.self) { ring in
                        Circle()
                            .stroke(tint.opacity(0.35 / Double(ring)), lineWidth: max(1, size * 0.012))
                            .frame(width: size * 0.5, height: size * 0.5)
                            .scaleEffect(1 + CGFloat(ring) * (0.18 + move * (face == .speaking ? 0.3 : 0.24)))
                    }
                }
                Circle()
                    .fill(RadialGradient(colors: [tint.opacity(face == .speaking ? 1 : 0.85), tint.opacity(face == .speaking ? 0.7 : 0.5)],
                                         center: .init(x: 0.35, y: 0.3), startRadius: 2, endRadius: size * 0.3))
                    .frame(width: size * 0.5, height: size * 0.5)
                    .scaleEffect((breath + move * (face == .speaking ? 0.18 : 0.06)))
                    .shadow(color: tint.opacity(face == .speaking ? 0.6 : 0.25), radius: face == .speaking ? size * 0.12 : size * 0.05)
            }
            .frame(width: size, height: size)
            .opacity(face == .muted ? 0.4 : 1)
            .animation(.easeOut(duration: 0.12), value: level)
            .animation(.easeInOut(duration: 0.25), value: face)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}
#endif
