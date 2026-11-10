import AppKit
import SwiftUI
import WheelKit

/// How a notice reads: green for news, orange for a warning, red for an error. The button colours
/// follow the level, so Restart Server, Copy and Focus pane all carry the colour of their notice.
enum ToastLevel: String {
    case notice, warning, error

    var color: Color {
        switch self {
        case .notice: .green
        case .warning: .orange
        case .error: .red
        }
    }

    var symbol: String {
        switch self {
        case .notice: "checkmark"
        case .warning: "exclamationmark"
        case .error: "xmark"
        }
    }
}

struct ToastAction: Identifiable {
    let id: String
    let title: String
    let run: @MainActor () -> Void
}

/// An inline reply: a text field under the message, sent to whoever the notice is about. No notice
/// carries one unless its source asks for it.
struct ToastReply {
    let placeholder: String
    let send: @MainActor (String) -> Void
}

/// The one shape of every notice Wheel shows: a message the person can select and copy, buttons for
/// what they can do about it, a close button, and sometimes a reply field.
struct Toast: Identifiable {
    let id: String
    var level: ToastLevel
    var text: String
    var actions: [ToastAction] = []
    var reply: ToastReply?
    /// The pane the notice is about; it gets Focus pane and Close pane buttons.
    var pane: String?
    /// What closing it does besides hiding it; nil when it cannot be closed (it clears itself).
    var dismiss: (@MainActor () -> Void)?
    var closable = true
    /// Draws a ring that empties over this span, beside the buttons.
    var countdown: ClosedRange<Date>?
}

extension AppModel {
    /// The most notices on screen at once; the newest wait behind them.
    static let toastLimit = 3

    /// Every notice now, in the order they stack from the top: the client and server updates, a
    /// lost connection, the daemon's last refusal, then what was posted (an agent's notice).
    var toasts: [Toast] {
        var list: [Toast] = []
        if let installed = clientInstalled, installed != clientUpdateDismissed {
            list.append(Toast(
                id: "client-update", level: .notice,
                text: "A newer Wheel is installed",
                actions: [ToastAction(id: "relaunch", title: "Relaunch Wheel") { [weak self] in self?.relaunchClient() }],
                dismiss: { [weak self] in self?.clientUpdateDismissed = installed }))
        }
        if serverBannerShown {
            list.append(Toast(
                id: "server-update", level: .notice,
                text: Self.serverUpdatedText,
                actions: serverControl?.unavailable == nil
                    ? [ToastAction(id: "restart", title: "Restart Server") { [weak self] in self?.restartServer() }] : [],
                dismiss: { [weak self] in self?.dismissServerBanner() }))
        }
        if let offer = rollbackOffer {
            list.append(Toast(
                id: "rollback", level: .warning, text: "Rolling back",
                actions: [ToastAction(id: "cancel", title: "Cancel") { [weak self] in self?.cancelRollback() }],
                closable: false, countdown: offer.range))
        } else if case let .unreachable(reason) = connection, !restartExpected {
            list.append(Toast(
                id: "unreachable", level: .warning,
                text: "wheeld unreachable: \(reason). Retrying every 2 seconds.", closable: false))
        }
        if let target = rolledBackTo, connection == .connected {
            list.append(Toast(
                id: "rolled-back", level: .notice, text: "Rolled back to \(target)",
                dismiss: { [weak self] in self?.rolledBackTo = nil }))
        }
        if let error = layoutError {
            list.append(Toast(id: "layout-error", level: .error, text: error, pane: Self.pane(named: error),
                              dismiss: { [weak self] in self?.layoutError = nil }))
        }
        return (list + posted).map(withPaneActions)
    }

    /// A notice about a pane that still exists gets Focus pane and Close pane after its own buttons.
    private func withPaneActions(_ toast: Toast) -> Toast {
        guard let pane = toast.pane, spaces.contains(where: { $0.allPanes.contains { $0.id == pane } }) else { return toast }
        var toast = toast
        toast.actions += [
            ToastAction(id: "focus", title: "Focus pane") { [weak self] in self?.show(pane: pane); self?.focus(pane: pane) },
            ToastAction(id: "close", title: "Close pane") { [weak self] in self?.askClosePane(pane) },
        ]
        return toast
    }

    /// The pane id a daemon message names, as in "agent w4653 in pane p4652 could not start".
    static func pane(named message: String) -> String? {
        guard let found = message.range(of: #"\bpane p\d+\b"#, options: .regularExpression) else { return nil }
        return String(message[found].dropFirst("pane ".count))
    }

    /// Show `toast`, replacing one with the same id.
    func post(_ toast: Toast) {
        posted.removeAll { $0.id == toast.id }
        posted.append(toast)
        Log.info("toast \(toast.id) posted: \(toast.level.rawValue), \(toast.actions.count) actions\(toast.reply == nil ? "" : ", reply")")
    }

    func removeToast(_ id: String) {
        posted.removeAll { $0.id == id }
    }

    /// The close button: whatever the toast does on close, then it is gone.
    func dismiss(_ toast: Toast) {
        Log.info("toast \(toast.id) dismissed")
        toast.dismiss?()
        removeToast(toast.id)
    }

    // MARK: The client

    /// When the running app's executable was last written. An install replaces the bundle, so the
    /// file at this path changing means the app on disk is not the one running.
    static func executableStamp() -> Date? {
        guard let path = Bundle.main.executableURL?.path else { return nil }
        return (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
    }

    /// Compare the executable on disk with the one this process started from.
    func checkClientUpdate() {
        guard let started = launchStamp, let now = Self.executableStamp() else { return }
        let installed = now == started ? nil : now
        guard installed != clientInstalled else { return }
        clientInstalled = installed
        Log.info("client: \(installed == nil ? "installed app matches the running one" : "a newer app is installed (started \(started), on disk \(now)); relaunch offered")")
    }

    /// Start the installed app and quit this one. Terminals and agents live in the server, so
    /// they stay up through it (ADR-0020).
    func relaunchClient() {
        let path = Bundle.main.bundlePath
        Log.info("client: relaunch asked for \(path)")
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.createsNewApplicationInstance = true
        NSWorkspace.shared.openApplication(at: URL(filePath: path), configuration: configuration) { _, error in
            Task { @MainActor in
                if let error { Log.info("client: relaunch failed: \(error)") } else { NSApp.terminate(nil) }
            }
        }
    }
}

/// The notices, bottom right over the window. They overlay the grid, so one coming or going moves
/// nothing under it.
struct ToastStack: View {
    var model: AppModel

    var body: some View {
        let toasts = model.toasts
        VStack(alignment: .trailing, spacing: 8) {
            ForEach(toasts.prefix(AppModel.toastLimit)) { toast in
                ToastView(toast: toast, dismiss: { model.dismiss(toast) })
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
        .task {
            // The app on disk changes under a running copy only at an install; looking every
            // 30 seconds is enough.
            while !Task.isCancelled {
                model.checkClientUpdate()
                try? await Task.sleep(for: .seconds(30))
            }
        }
    }
}

struct ToastView: View {
    let toast: Toast
    let dismiss: () -> Void
    @State private var reply = ""

    /// A short message with at most one button sits on one line, as a pill.
    private var compact: Bool {
        toast.text.count <= 60 && toast.actions.count <= 1 && toast.reply == nil
    }

    private var tint: Color { toast.level.color }

    var body: some View {
        Group {
            if compact {
                HStack(spacing: 10) {
                    badge
                    message.lineLimit(1)
                    if let countdown = toast.countdown { ring(countdown) }
                    ForEach(toast.actions) { action in button(action.title, action.run) }
                    if toast.countdown == nil { copyButton(labelled: false) }
                    if toast.closable { closeButton }
                }
                .padding(.leading, 12).padding(.trailing, 7).padding(.vertical, 7)
                .background(.regularMaterial, in: Capsule())
                .overlay(Capsule().strokeBorder(.separator))
            } else {
                HStack(alignment: .top, spacing: 10) {
                    badge.padding(.top, 1)
                    VStack(alignment: .leading, spacing: 8) {
                        message.lineLimit(6)
                        if let inline = toast.reply { replyField(inline) }
                        HStack(spacing: 6) {
                            ForEach(toast.actions) { action in button(action.title, action.run) }
                            copyButton(labelled: true)
                        }
                    }
                    if toast.closable { closeButton }
                }
                .padding(.leading, 12).padding(.trailing, 8).padding(.vertical, 10)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(.separator))
            }
        }
        .frame(maxWidth: 460, alignment: .trailing)
        .fixedSize(horizontal: false, vertical: true)
        .shadow(color: .black.opacity(0.3), radius: 10, y: 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("toast.\(toast.id)")
    }

    /// A ring that empties as `span` runs out, with the whole seconds left inside it.
    private func ring(_ span: ClosedRange<Date>) -> some View {
        TimelineView(.animation) { context in
            let total = span.upperBound.timeIntervalSince(span.lowerBound)
            let left = max(0, span.upperBound.timeIntervalSince(context.date))
            ZStack {
                Circle().stroke(tint.opacity(0.18), lineWidth: 3)
                Circle().trim(from: 0, to: total > 0 ? left / total : 0)
                    .stroke(tint, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                Text("\(Int(left.rounded(.up)))").font(.system(size: 10, weight: .semibold)).monospacedDigit()
            }
            .frame(width: 24, height: 24)
        }
        .accessibilityIdentifier("toast.\(toast.id).countdown")
    }

    private var badge: some View {
        Image(systemName: toast.level.symbol)
            .font(.system(size: 9, weight: .heavy))
            .foregroundStyle(.black.opacity(0.75))
            .frame(width: 18, height: 18)
            .background(tint, in: Circle())
            .accessibilityLabel(toast.level.rawValue)
    }

    /// Selectable, so any of it can be copied by hand; the Copy button takes all of it.
    private var message: some View {
        Text(toast.text)
            .font(.system(size: 13))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func button(_ title: String, _ run: @escaping () -> Void) -> some View {
        Button(action: run) {
            Text(title).font(.system(size: 12.5, weight: .medium))
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(tint.opacity(0.18), in: Capsule())
                .foregroundStyle(tint)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("toast.\(toast.id).\(title)")
    }

    private func copyButton(labelled: Bool) -> some View {
        Button { Self.copy(toast.text) } label: {
            Group {
                if labelled {
                    Text("Copy").font(.system(size: 12.5, weight: .medium)).padding(.horizontal, 10)
                } else {
                    Image(systemName: "doc.on.doc").font(.system(size: 11)).frame(width: 24)
                }
            }
            .padding(.vertical, 4)
            .background(tint.opacity(0.18), in: Capsule())
            .foregroundStyle(tint)
        }
        .buttonStyle(.plain)
        .help("Copy this message")
        .accessibilityLabel("Copy")
        .accessibilityIdentifier("toast.\(toast.id).Copy")
    }

    private var closeButton: some View {
        Button(action: dismiss) {
            Image(systemName: "xmark").font(.system(size: 10, weight: .semibold))
                .frame(width: 24, height: 24)
                .background(Color.primary.opacity(0.1), in: Circle())
        }
        .buttonStyle(.plain)
        .help("Dismiss")
        .accessibilityLabel("Dismiss")
        .accessibilityIdentifier("toast.\(toast.id).Dismiss")
    }

    private func replyField(_ inline: ToastReply) -> some View {
        HStack(spacing: 6) {
            TextField(inline.placeholder, text: $reply)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .padding(.horizontal, 8).padding(.vertical, 5)
                .background(Color.primary.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
                .onSubmit { sendReply(inline) }
            button("Send") { sendReply(inline) }
        }
    }

    private func sendReply(_ inline: ToastReply) {
        let text = reply.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        inline.send(text)
        reply = ""
    }

    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        Log.info("toast: copied \(text.count) characters")
    }
}
