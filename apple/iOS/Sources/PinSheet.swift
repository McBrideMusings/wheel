import SwiftUI
import WheelKit

/// The shown space's pinned pane as a bottom sheet over the carousel. It sits full (everything under the top bar),
/// high, low or as its header alone at the bottom edge, so it never leaves the screen; a drag on its header moves it
/// and ends at the nearest of the four. The header names the pane, and its menu switches among the space's pinned
/// panes and unpins.
struct PinSheetView<Content: View>: View {
    let model: PhoneModel
    let space: SpaceInfo
    let sheet: PinSheet
    let pane: (id: String, kind: PaneKind)
    @ViewBuilder let content: ((id: String, kind: PaneKind)) -> Content
    /// How far the header has been dragged up, in points, while a drag lasts. Plain state, not a gesture state: a
    /// gesture state snaps to zero outside any animation as the finger lifts, so the sheet would jump back to where
    /// the drag began for a frame before springing to where it ends.
    @State private var lift: CGFloat = 0

    /// Whether the keyboard is up. The keyboard takes the bottom of the screen, so a sheet at low or below would
    /// leave its composer with no room for the chat above it: while it is up the sheet sits full, and it goes back
    /// to its own position when the keyboard goes down.
    @State private var keyboard = false

    static var headerHeight: CGFloat { 44 }

    var body: some View {
        GeometryReader { geo in
            let total = geo.size.height
            let settled = (keyboard ? SheetPosition.full : sheet.position).fraction * total
            let height = min(max(settled + lift, Self.headerHeight), total)
            ZStack(alignment: .bottomTrailing) {
                Color.clear
                VStack(spacing: 0) {
                    header(total: total)
                    content(pane)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .clipped()
                }
                // Top-aligned: a pane's content (a composer) has a least height, and centred overflow would push
                // the header out of a sheet down to its header alone.
                .frame(height: height, alignment: .top)
                .background(Color(.systemBackground))
                .clipShape(UnevenRoundedRectangle(topLeadingRadius: 12, topTrailingRadius: 12))
                .shadow(color: .black.opacity(0.18), radius: 8, y: -2)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("pin sheet")
        .accessibilityValue(sheet.position.rawValue)
        .onChange(of: sheet.position, initial: true) { _, position in Log.info("pin sheet: \(position.rawValue), showing \(sheet.pane)") }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillShowNotification)) { _ in
            Log.info("pin sheet: keyboard up, full")
            withAnimation(.spring(response: 0.3, dampingFraction: 1)) { keyboard = true }
        }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in
            Log.info("pin sheet: keyboard down, \(sheet.position.rawValue)")
            withAnimation(.spring(response: 0.3, dampingFraction: 1)) { keyboard = false }
        }
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
            .onChanged { drag in lift = -drag.translation.height }
            .onEnded { drag in
                // Where the flick is heading, not where the finger stopped, picks the position; the same spring that
                // moves the sheet there clears the lift, so it glides from where it was let go.
                let shown = keyboard ? SheetPosition.full : sheet.position
                let end = shown.fraction * total - drag.predictedEndTranslation.height
                let position = SheetPosition.nearest(to: end, in: total)
                // Pulled down while typing: the person wants the pages back, so the keyboard goes and the sheet
                // settles where the drag ends.
                if keyboard && position != .full {
                    UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
                }
                Log.info("pin sheet: dragged \(Int(-drag.translation.height)) pt from \(sheet.position.rawValue), ends \(position.rawValue)")
                withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                    model.moveSheet(to: position)
                    lift = 0
                }
            })
    }

    private func title(of id: String) -> String { model.pageName(id, in: space) }
}
