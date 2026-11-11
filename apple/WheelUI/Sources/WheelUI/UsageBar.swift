import SwiftUI

/// A rounded bar filled to `pct`: neutral, orange from 80%, red from 95%, track-coloured when stale.
/// `pace`, 0 to 1, draws a tick at that share of the bar: how much of the window has passed, so a
/// fill ahead of the tick is spending faster than the window.
public struct UsageBar: View {
    let pct: Double
    let stale: Bool
    let pace: Double?

    public init(pct: Double, stale: Bool, pace: Double? = nil) {
        self.pct = pct
        self.stale = stale
        self.pace = pace
    }

    private var fill: Color {
        if stale { return .primary.opacity(0.12) }
        if pct >= 95 { return .red }
        if pct >= 80 { return .orange }
        return .primary.opacity(0.55)
    }

    public var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(.primary.opacity(0.12))
                Capsule().fill(fill)
                    .frame(width: max(geo.size.height, geo.size.width * min(max(pct, 0), 100) / 100))
                if let pace {
                    Rectangle().fill(.primary.opacity(0.7))
                        .frame(width: 1.5, height: geo.size.height + 4)
                        .offset(x: (geo.size.width - 1.5) * min(max(pace, 0), 1), y: -2)
                        .accessibilityHidden(true)
                }
            }
        }
    }
}
