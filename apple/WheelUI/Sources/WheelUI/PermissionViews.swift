import SwiftUI
import WheelKit

/// The three answers a permission card offers, in the order its buttons sit.
public enum PermissionKey {
    case deny, always, allow

    /// The request's option for this answer, when it offers one: Deny is any refusal, Always allow
    /// the agent's own allow-always option, Allow the one-time allow.
    public func option(in perm: PermInfo) -> PermOption? {
        switch self {
        case .deny: perm.options.first { $0.kind.hasPrefix("reject") }
        case .always: perm.options.first { $0.kind == "allow_always" }
        case .allow: perm.options.first { $0.kind == "allow_once" } ?? perm.options.first { !$0.kind.hasPrefix("reject") && $0.kind != "allow_always" }
        }
    }
}

/// Why the agent asks: its own one-line reason, then what made it ask and the path involved,
/// in plain text above the command.
public struct PermissionWhy: View {
    let perm: PermInfo

    public init(perm: PermInfo) { self.perm = perm }

    public var body: some View {
        if perm.reason != nil || perm.decisionReason != nil || perm.blockedPath != nil {
            VStack(alignment: .leading, spacing: 3) {
                if let reason = perm.reason {
                    Text(reason).font(ChatFont.line)
                }
                if let why = perm.decisionReason {
                    Text(why).font(ChatFont.meta).foregroundStyle(.secondary)
                }
                if let path = perm.blockedPath {
                    Text("Path: \(path)").font(ChatFont.meta).foregroundStyle(.secondary)
                }
            }
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// A request's choices in one group at the right: the refusals, then the allowances that last,
/// then the one-time allow filled, as the default. With `keys`, ⌘↩ always allows; Return allows and
/// Esc denies through the composer (`Composer.permissionKey`), and each tooltip names its key.
public struct PermissionChoices: View {
    let perm: PermInfo
    let answer: (PermInfo, String) -> Void
    let keys: Bool

    public init(perm: PermInfo, answer: @escaping (PermInfo, String) -> Void, keys: Bool) {
        self.perm = perm
        self.answer = answer
        self.keys = keys
    }

    /// The tooltip a button shows: its name, what it adds when it adds a rule, and its key.
    public static func help(_ option: PermOption, in perm: PermInfo, keys: Bool) -> String {
        let key: String
        switch option.kind {
        case "allow_once": key = keys ? " (↩)" : ""
        case "allow_always": key = keys ? " (⌘↩)" : ""
        case "reject_once": key = keys ? " (Esc)" : ""
        default: key = ""
        }
        guard option.kind == "allow_always", let rule = perm.alwaysRule else { return option.name + key }
        return "\(option.name): adds the rule \(rule)\(key)"
    }

    public var body: some View {
        let allow = PermissionKey.allow.option(in: perm)
        let rejects = perm.options.filter { $0.kind.hasPrefix("reject") }
        let middle = perm.options.filter { !$0.kind.hasPrefix("reject") && $0.id != allow?.id }
        HStack(spacing: 8) {
            Spacer(minLength: 8)
            ForEach(rejects, id: \.id) { option in
                Button(option.name) { answer(perm, option.id) }
                    .help(Self.help(option, in: perm, keys: keys))
            }
            ForEach(middle, id: \.id) { option in
                Button(option.name) { answer(perm, option.id) }
                    .keyboardShortcut(keys && option.kind == "allow_always" ? KeyboardShortcut(.return, modifiers: .command) : nil)
                    .help(Self.help(option, in: perm, keys: keys))
            }
            if let allow {
                Button(allow.name) { answer(perm, allow.id) }
                    .buttonStyle(.borderedProminent)
                    .help(Self.help(allow, in: perm, keys: keys))
            }
        }
        .controlSize(.small)
    }
}
