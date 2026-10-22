import SwiftUI

/// A rounded bar filled to `pct`: neutral, orange from 80%, red from 95%, track-coloured when stale.
public struct UsageBar: View {
    let pct: Double
    let stale: Bool

    public init(pct: Double, stale: Bool) {
        self.pct = pct
        self.stale = stale
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
            }
        }
    }
}
