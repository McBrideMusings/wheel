import SwiftUI
import WheelKit

/// The shown space's pinned pane as a bottom sheet over the carousel. It sits full (everything under the top bar),
/// high, low or hidden below the bottom edge; a drag on its header moves it and ends at the nearest of the four, and
/// while it is hidden a round button brings it back at low. The header names the pane, and its menu switches among the
/// space's pinned panes and unpins.
struct PinSheetView<Content: View>: View {
    let model: PhoneModel
    let space: SpaceInfo
    let sheet: PinSheet
    let pane: (id: String, kind: PaneKind)
    @ViewBuilder let content: ((id: String, kind: PaneKind)) -> Content
    /// How far the header has been dragged up, in points, while a drag lasts.
    @GestureState private var lift: CGFloat = 0

    static var headerHeight: CGFloat { 44 }

    var body: some View {
        GeometryReader { geo in
            let total = geo.size.height
            let settled = sheet.position.fraction * total
            let height = min(max(settled + lift, sheet.position == .hidden ? 0 : Self.headerHeight), total)
            ZStack(alignment: .bottomTrailing) {
                Color.clear
                if sheet.position == .hidden && lift == 0 {
                    showButton
                } else {
                    VStack(spacing: 0) {
                        header(total: total)
                        content(pane)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .clipped()
                    }
                    .frame(height: height)
                    .background(Color(.systemBackground))
                    .clipShape(UnevenRoundedRectangle(topLeadingRadius: 12, topTrailingRadius: 12))
                    .shadow(color: .black.opacity(0.18), radius: 8, y: -2)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("pin sheet")
        .accessibilityValue(sheet.position.rawValue)
        .onChange(of: sheet.position, initial: true) { _, position in Log.info("pin sheet: \(position.rawValue), showing \(sheet.pane)") }
    }

    /// The grabber, the pane's name with the pinned-pane menu, and the drag that moves the sheet.
    private func header(total: CGFloat) -> some View {
        VStack(spacing: 4) {
            Capsule().fill(Color.secondary.opacity(0.5)).frame(width: 36, height: 5).padding(.top, 6)
            HStack(spacing: 8) {
                Menu {
                    let pinned = model.pins.pinned(in: space.id)
                    ForEach(pinned, id: \.self) { id in
                        Button {
                            model.showPinned(id)
                        } label: {
                            if id == pane.id { Label(title(of: id), systemImage: "checkmark") } else { Text(title(of: id)) }
                        }
                    }
                    Divider()
                    Button("Unpin", systemImage: "pin.slash") { model.unpin(pane.id) }
                } label: {
                    HStack(spacing: 4) {
                        Text(title(of: pane.id)).font(.subheadline.weight(.semibold)).lineLimit(1)
                        Image(systemName: "chevron.down").font(.caption2.weight(.semibold))
                    }
                    .foregroundStyle(.primary)
                }
                .accessibilityIdentifier("pin sheet menu")
                .accessibilityValue(title(of: pane.id))
                Spacer()
            }
            .padding(.horizontal, 16)
        }
        .frame(height: Self.headerHeight, alignment: .top)
        .frame(maxWidth: .infinity)
        .contentShape(Rectangle())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("pin sheet header")
        // Measured on the screen: the header rises with the sheet, so its own space would shrink the drag under the finger.
        .gesture(DragGesture(minimumDistance: 8, coordinateSpace: .global)
            .updating($lift) { drag, lift, _ in lift = -drag.translation.height }
            .onEnded { drag in
                let end = sheet.position.fraction * total - drag.translation.height
                let position = SheetPosition.nearest(to: end, in: total)
                Log.info("pin sheet: dragged \(Int(-drag.translation.height)) pt from \(sheet.position.rawValue), ends \(position.rawValue)")
                withAnimation(.snappy) { model.moveSheet(to: position) }
            })
    }

    /// While the sheet is hidden: a round button over the carousel's bottom right that brings it back at low.
    private var showButton: some View {
        Button {
            Log.info("pin sheet: show button tapped")
            withAnimation(.snappy) { model.moveSheet(to: .low) }
        } label: {
            Image(systemName: "pin.fill")
                .font(.title3)
                .frame(width: 48, height: 48)
                .background(.regularMaterial, in: Circle())
                .shadow(color: .black.opacity(0.2), radius: 4, y: 1)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Show pinned pane")
        .accessibilityIdentifier("sheet show")
        // Above the dots bar and a page's bottom controls (a terminal's key row, an agent's composer), so it covers none.
        .padding(.trailing, 16)
        .padding(.bottom, 152)
    }

    private func title(of id: String) -> String {
        let kind = space.allPanes.first { $0.id == id }?.kind
        return model.mirror(id)?.title ?? space.paneNames[id] ?? kind.flatMap { PhoneCommands.paneName($0, status: nil) } ?? kind?.name ?? id
    }
}
