import SwiftUI
import WheelKit
#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// What a pane is, as the daemon lists it: each field's label, its value in a monospaced face, the
/// hint under it, and a button that copies the value. The Mac's ⓘ popover and the phone's pane
/// menu show the same list.
public struct PaneInfoView: View {
    let fields: [InfoField]?

    public init(fields: [InfoField]?) { self.fields = fields }

    public var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Pane info").font(.headline)
            if let fields {
                ForEach(fields) { field in
                    HStack(alignment: .top, spacing: 12) {
                        Text(field.label)
                            .foregroundStyle(.secondary)
                            .frame(width: 84, alignment: .leading)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(field.value)
                                .font(.system(.callout, design: .monospaced))
                                .textSelection(.enabled)
                            if let hint = field.hint {
                                Text(hint).font(.caption).foregroundStyle(.tertiary)
                            }
                        }
                        Spacer(minLength: 4)
                        Button {
                            Self.copy(field.value)
                        } label: {
                            Image(systemName: "doc.on.doc")
                        }
                        .buttonStyle(.borderless)
                        .accessibilityLabel("Copy \(field.label)")
                    }
                }
            } else {
                ProgressView().controlSize(.small)
            }
        }
        .padding(14)
    }

    /// Put `text` on the system clipboard.
    public static func copy(_ text: String) {
        #if canImport(AppKit)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        #elseif canImport(UIKit)
        UIPasteboard.general.string = text
        #endif
    }
}
