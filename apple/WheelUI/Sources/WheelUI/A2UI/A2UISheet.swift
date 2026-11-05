import A2UISwiftCore
import A2UISwiftUI
import SwiftUI

// MARK: Sheet — a dialog over the page while `presented`: children[0] is its body, children[1] its actions bar; closing raises `action`.
// The system presents it (so the page's catalog reaches the content): a header with Close, a body that scrolls and the actions bar under it.
struct A2UISheetView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let binding = Binding<Bool>(
            get: { p["presented"]?.boolValue ?? false },
            set: { open in
                if !open, let name = p["action"]?.stringValue {
                    surface.dispatchAction(name: name, sourceComponentId: node.instance.id)
                }
            })
        Color.clear.frame(width: 0, height: 0)
            .sheet(isPresented: binding) { content(close: { binding.wrappedValue = false }) }
    }

    @ViewBuilder
    private func content(close: @escaping () -> Void) -> some View {
        let t = WebTheme.shared
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button(action: close) {
                    Text("Close").font(.system(size: WebTheme.callout)).foregroundStyle(t.label)
                        .padding(.horizontal, 10).frame(height: 26)
                        .background(t.group, in: RoundedRectangle(cornerRadius: 6))
                        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(t.separatorStrong, lineWidth: 1))
                }
                .buttonStyle(.plain)
                .keyboardShortcut(.cancelAction)
            }
            .padding(.leading, 16).padding(.trailing, 12).padding(.vertical, 8)
            .frame(minHeight: 44)
            .overlay(alignment: .bottom) { Rectangle().fill(t.separator).frame(height: 1) }
            ScrollView {
                if let c = node.children.first {
                    A2UIChildView(node: c, surface: surface)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 14).padding(.horizontal, 18).padding(.bottom, 20)
                }
            }
            if node.children.count > 1 {
                A2UIChildView(node: node.children[1], surface: surface)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12).padding(.vertical, 10)
                    .overlay(alignment: .top) { Rectangle().fill(t.separator).frame(height: 1) }
            }
        }
        .background(t.group)
        #if os(macOS)
        .frame(minWidth: 480, idealWidth: 640, minHeight: 520, idealHeight: 720)
        #endif
    }
}
