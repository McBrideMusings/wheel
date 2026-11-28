import AppKit
import Combine
import WebKit
import IOSurface
import QuartzCore
import GhosttyTerminal
import Observation
import SwiftUI
import WheelKit
import WheelUI

// The native half of the GPUI client: artifact pages' WKWebViews and plugin views, one per pane id,
// each in the slot view the Rust pane host made for the pane (clients/gpui/src/appkit/pane_host.rs),
// which places, masks and focuses it; browser and plain web panes are made there too (web.rs).
// Every C entrypoint is called on the main thread. Panes are opened and closed by JSON commands;
// the Rust side owns what each pane is.

@MainActor final class PaneHost {
    let id: String
    let kind: String
    /// The slot view the Rust pane host made for the pane: the content goes in it, and the host places, masks and focuses it.
    let slot: NSView
    var web: WKWebView?
    var url: String?
    var loads: [String] = []
    /// An artifact page: the address under `WebPanes.scheme` it opens at, how many times a
    /// `pane_reload` opened it again, the HTTP status of the last document, the store its
    /// browser storage lives in, and its visible text after the last load.
    var pageURL: URL?
    var reloads = 0
    var status: Int?
    var store: UUID?
    var text: String?
    /// The artifact the page belongs to, as `<scope>/<name>`, and the space directory it was opened
    /// for: its store and scheme handler are bound to both, so another of either is another view.
    var artifact: String?
    var cwd: String?
    /// A plugin pane (ADR-0041): its Luau script drawn as WheelUI components, and the plugin and pane it runs, as wheeld names them.
    var plugin: PluginPane?
    var pluginView: NSHostingView<AnyView>?
    var pluginLabel: String?
    init(id: String, kind: String, slot: NSView) { self.id = id; self.kind = kind; self.slot = slot }
    var content: NSView { pluginView ?? web! }
}

/// A pane's terminal drawn inside a plugin pane's page (`ui.Terminal{pane}`, ADR-0042): its own session and
/// surface on the term the pane holds, the page's size its grid. It shows a message instead when the pane is
/// gone or holds no terminal, and attaches again when `pane_targets` says the pane has one.
@MainActor final class EmbedTerminal: NSView {
    var pane: String
    let hostPane: String
    unowned let native: Native
    var termID: String?
    var session: InMemoryTerminalSession?
    var terminal: DropTerminalView?
    var bytes = 0
    /// The last grid this embed sent as `term_resize`.
    var lastSent: [Int]?
    let note = NSTextField(labelWithString: "")
    init(native: Native, hostPane: String, pane: String) {
        self.native = native; self.hostPane = hostPane; self.pane = pane
        super.init(frame: .zero)
        note.alignment = .center
        note.textColor = .secondaryLabelColor
        note.lineBreakMode = .byTruncatingTail
        addSubview(note)
        resolve()
    }
    required init?(coder: NSCoder) { fatalError() }
    override func layout() {
        super.layout()
        terminal?.frame = bounds
        let h = note.fittingSize.height
        note.frame = NSRect(x: 0, y: (bounds.height - h) / 2, width: bounds.width, height: h)
    }
    /// Stop showing the term: remove the surface and release the term (idempotent, the second call finds no term).
    func drop() {
        let old = termID
        terminal?.removeFromSuperview()
        terminal = nil; session = nil; termID = nil; lastSent = nil
        if let old { native.release(old, except: self) }
    }
    /// The term the pane holds now, attached; the old one detached when nothing else uses it.
    func resolve() {
        let term = native.paneTargets[pane]?["term"]
        // The message follows every resolution, the first included, whether or not the term changed.
        note.stringValue = term != nil ? "" : native.paneTargets[pane] == nil ? "Pane \(pane) is gone" : "Pane \(pane) has no terminal"
        note.isHidden = term != nil
        needsLayout = true
        guard term != termID else { return }
        drop()
        guard let term else { return }
        termID = term
        if let ws = native.pluginURL { native.connect(ws) }
        let session = InMemoryTerminalSession(write: { [weak self] data in
            Task { @MainActor in
                guard let self, self.termID == term else { return }
                self.native.send(["op": "term_input", "id": term, "data": data.base64EncodedString()])
            }
        }, resize: { [weak self] viewport in
            Task { @MainActor in
                guard let self, self.termID == term else { return }
                let grid = [Int(viewport.columns), Int(viewport.rows)]
                guard self.lastSent != grid else { return }
                self.lastSent = grid
                self.native.send(["op": "term_resize", "id": term, "cols": grid[0], "rows": grid[1]])
            }
        })
        let view = DropTerminalView(frame: bounds)
        view.controller = native.embedController
        view.configuration = TerminalSurfaceOptions(backend: .inMemory(session))
        self.session = session
        terminal = view
        addSubview(view)
        native.send(["op": "term_attach", "id": term])
        needsLayout = true
    }
}

/// `EmbedTerminal` for SwiftUI; its registration on `Native` lasts as long as the view.
struct EmbeddedTerminal: NSViewRepresentable {
    let native: Native
    let hostPane: String
    let pane: String
    func makeNSView(context: Context) -> EmbedTerminal {
        let view = EmbedTerminal(native: native, hostPane: hostPane, pane: pane)
        native.embeds[ObjectIdentifier(view)] = view
        return view
    }
    func updateNSView(_ view: EmbedTerminal, context: Context) {
        guard view.pane != pane else { return }
        view.pane = pane
        view.resolve()
    }
    static func dismantleNSView(_ view: EmbedTerminal, coordinator: ()) {
        view.native.embeds[ObjectIdentifier(view)] = nil
        view.drop()
    }
}

/// A pane's agent conversation inside a plugin pane's page (`ui.AgentChat{pane}`, ADR-0042): the agent the pane holds,
/// as `pane_targets` last said, its `AgentChat` shared with every other embed of that agent. A pane that is gone or holds
/// no agent leaves `chat` nil and says so in `note`; the agent comes back when `pane_targets` names one again.
@MainActor final class EmbedAgent: ObservableObject {
    unowned let native: Native
    let hostPane: String
    var pane: String
    @Published private(set) var chat: AgentChat?
    @Published private(set) var note = ""
    init(native: Native, hostPane: String, pane: String) {
        self.native = native; self.hostPane = hostPane; self.pane = pane
    }
    /// The agent the pane holds now, attached; the old one detached when no other embed shows it.
    func resolve() {
        let agent = native.paneTargets[pane]?["agent"]
        let note = agent != nil || !native.targetsKnown ? "" : native.paneTargets[pane] == nil ? "Pane \(pane) is gone" : "Pane \(pane) holds no agent"
        if self.note != note { self.note = note }
        guard agent != chat?.name else { return }
        drop()
        guard let agent else { return }
        chat = native.agentChat(agent)
    }
    /// Stop showing the agent (idempotent).
    func drop() {
        guard let old = chat else { return }
        chat = nil
        native.releaseChat(old.name)
    }
}

/// The waiting permission requests the page connection last heard, for every agent embed.
@MainActor final class EmbedPermissions: ObservableObject {
    @Published var list: [PermInfo] = []
}

/// `EmbedAgent` for SwiftUI: the conversation, its request and a composer while the pane holds an agent, the note otherwise.
/// Its registration on `Native` lasts while the view is shown.
struct EmbeddedAgentChat: View {
    let native: Native
    let pane: String
    @StateObject private var embed: EmbedAgent
    @ObservedObject private var perms: EmbedPermissions
    init(native: Native, hostPane: String, pane: String) {
        self.native = native
        self.pane = pane
        _embed = StateObject(wrappedValue: EmbedAgent(native: native, hostPane: hostPane, pane: pane))
        perms = native.embedPerms
    }
    var body: some View {
        Group {
            if let chat = embed.chat {
                EmbeddedAgentBody(native: native, chat: chat, perms: perms.list)
            } else {
                Text(embed.note)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear {
            native.agentEmbeds[ObjectIdentifier(embed)] = embed
            embed.resolve()
        }
        .onDisappear {
            native.agentEmbeds[ObjectIdentifier(embed)] = nil
            embed.drop()
        }
        .onChange(of: pane) {
            embed.pane = pane
            embed.resolve()
        }
    }
}

/// One agent's conversation as the Mac's agent pane lays it out: the transcript, its running tasks, the notice the
/// daemon holds, the first request it waits on, and the composer.
struct EmbeddedAgentBody: View {
    let native: Native
    @ObservedObject var chat: AgentChat
    let perms: [PermInfo]
    var body: some View {
        let mine = perms.filter { $0.agent == chat.name }
        VStack(spacing: 0) {
            TranscriptView(
                chat: chat,
                empty: "Give this agent a task.",
                permissions: PermissionQueue(
                    waiting: Set(mine.compactMap(\.toolCall)),
                    places: Dictionary(perms.enumerated().compactMap { index, perm in perm.toolCall.map { ($0, index + 1) } },
                                       uniquingKeysWith: { first, _ in first }),
                    count: perms.count))
                .overlay { TaskFullView(chat: chat) { native.readTaskOutput(chat, task: $0) } }
            RunningStrip(chat: chat)
                .padding([.horizontal, .top], 8)
            if let notice = chat.notice {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: "info.circle").foregroundStyle(.secondary)
                    Text(notice).font(.footnote).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
                    Button("Dismiss", systemImage: "xmark") { native.pageClient?.send(.agentNoticeDismiss(req: nil, agent: chat.name)) }
                        .labelStyle(.iconOnly).buttonStyle(.borderless)
                }
                .padding([.horizontal, .top], 8)
            }
            if let perm = mine.first { PermissionCard(native: native, perm: perm) }
            Composer(
                placeholder: "Message \(AgentInfo.title(of: chat.name))",
                draft: $chat.draft,
                error: chat.error,
                enabled: true,
                send: { native.sendAgent(chat) },
                busy: chat.isBusy,
                stop: { native.pageClient?.send(.agentCancel(req: nil, agent: chat.name)) },
                takeBack: { chat.takeBack() })
                .padding(8)
        }
    }
}

/// A waiting permission request as an agent chat embed draws it: why, the command, the choices.
struct PermissionCard: View {
    let native: Native
    let perm: PermInfo
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            PermissionWhy(perm: perm)
            Text(perm.title)
                .font(perm.isQuestion ? .callout : .callout.monospaced())
                .lineLimit(4)
            if perm.isQuestion {
                QuestionChoices(perm: perm, answer: native.answer)
            } else {
                PermissionChoices(perm: perm, answer: native.answer, keys: false)
            }
        }
        .padding(8)
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Color.orange.opacity(0.6)))
        .padding([.horizontal, .top], 8)
    }
}

/// A terminal that takes dropped files: each file's path, shell-escaped, pasted at the prompt as
/// Ghostty's own Mac app does (its surface view's drop handler), several separated by a space.
final class DropTerminalView: AppTerminalView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.fileURL])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { Self.paths(sender).isEmpty ? [] : .copy }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool { drop(Self.paths(sender)) }
    static func paths(_ sender: NSDraggingInfo) -> [String] {
        let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
        return urls?.map(\.path) ?? []
    }
    /// Paste `paths` as the shell reads them; false when there are none or no surface yet.
    @discardableResult func drop(_ paths: [String]) -> Bool {
        guard !paths.isEmpty else { return false }
        return paste(text: paths.map(Self.escaped).joined(separator: " "))
    }
    /// A backslash before each character a shell would read as syntax (Ghostty's `shellEscape` set).
    static let shellSpecial = Set("\\ ()[]{}<>\"'`!#$&;|*?\t")
    static func escaped(_ path: String) -> String {
        var out = ""
        for c in path {
            if shellSpecial.contains(c) { out.append("\\") }
            out.append(c)
        }
        return out
    }
}

/// A frame the bridge sends the Rust side on its own initiative: the C callback `attach` was
/// given, with the bridge's window id and one JSON object.
public typealias Emit = @convention(c) (Int32, UnsafePointer<CChar>) -> Void

@MainActor final class Native: NSObject, WKNavigationDelegate {
    let root: NSView
    let window: Int32
    let emitter: Emit
    /// The one Ghostty controller every embedded terminal uses, in the default look.
    lazy var embedController: TerminalController = TerminalStyle.controller(.standard)
    var panes: [String: PaneHost] = [:]
    /// Every pane and what it holds, as the Rust side last said (`pane_targets`): `term`, `agent`.
    var paneTargets: [String: [String: String]] = [:]
    /// The terminals plugin pages embed, by object identity.
    var embeds: [ObjectIdentifier: EmbedTerminal] = [:]
    /// The agent chats plugin pages embed, by object identity, and the one `AgentChat` each agent they show streams into.
    var agentEmbeds: [ObjectIdentifier: EmbedAgent] = [:]
    var chats: [String: AgentChat] = [:]
    /// How many embeds hold each agent's chat: the last to let go detaches it.
    var chatHolds: [String: Int] = [:]
    /// The agents whose attach wheeld refused (the agent was not there yet): attached again when wheeld next names them.
    var refusedAttach: Set<String> = []
    var attachReqs: Set<UInt64> = []
    /// Whether a `pane_targets` has come yet: until it has, an embed says nothing rather than "gone".
    var targetsKnown = false
    /// Each agent request waiting for its reply, by req, with the agent it was for: an error reply goes on that chat.
    var agentReqs: [UInt64: String] = [:]
    let embedPerms = EmbedPermissions()
    var socket: URLSessionWebSocketTask?
    /// The connection plugin panes' embeds talk over (agent chats, artifacts), started with the first one and dropped with `disconnect`.
    var pageClient: DaemonClient?
    /// Where the daemon is and the token it wants, for every artifact page's scheme handler.
    let pageDaemon = PageDaemon()
    /// The artifacts plugin pages embed (`WebView{artifact}`), over the page connection.
    var artifactEmbeds: ArtifactEmbeds?
    var videoStreams: VideoStreams?
    /// The plugin panes' own connection to wheeld: it asks for each pane's script and hears every change to one. Unlike the
    /// terminal socket it is opened again when it drops, and asks for every script again, as long as a plugin pane exists;
    /// the last plugin pane closing closes it.
    var pluginSocket: URLSessionWebSocketTask?
    var pluginURL: String?
    var pluginReqs: [Int: String] = [:]
    /// Each `feed_subscribe` waiting for its reply, with the pane, feed and params that sent it.
    var feedReqs: [Int: (pane: String, feed: String, params: JSONValue)] = [:]
    /// Each `plugin_action` waiting for its reply, with the pane and action that sent it.
    var actionReqs: [Int: (pane: String, action: String)] = [:]
    var nextPluginReq = 1
    /// Seconds to wait before the next reconnect: 1, doubling to 30, back to 1 once wheeld answers.
    var pluginBackoff: UInt64 = 1
    var sendTask: Task<Void, Never>?
    var errors: [String] = []
    var heldBeforeKey: String?
    var termOutputs = 0
    var sends: [String] = []
    var monitor: Any?
    /// The chords the GPUI view runs an action for now, in wheeld's spelling (`keymap` op).
    var routed: Set<String> = []


    init(root: NSView, window: Int32, hidden: Bool, emit: @escaping Emit) {
        self.root = root
        self.window = window
        self.emitter = emit
        super.init()
        Self.keyEquivalentsFollowTheKeyboard(root)
        // Test mode: no Dock icon, activation, ordering or key-window call, deaf to the pointer.
        if hidden {
            NSApp.setActivationPolicy(.prohibited)
            root.window?.ignoresMouseEvents = true
        }
        // Hardware keys reach the focused native view first; the pane-cycling shortcuts are the
        // GPUI view's, so a native pane hands the window's keyboard back to it for those.
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.routeKey(event)
            return event
        }
    }
    /// Send one JSON object to the Rust side: a plugin pane's state, or the answer to a daemon
    /// request the Rust side relayed.
    func emit(_ value: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: value), let string = String(data: data, encoding: .utf8) else { return }
        string.withCString { emitter(window, $0) }
    }

    /// A chord the GPUI view's keymap runs now belongs to it, wherever the keyboard is, as the
    /// Swift app's key monitor runs its keymap before the pane sees the key: make the GPUI view the
    /// first responder before the key lands.
    func routeKey(_ event: NSEvent) {
        guard event.window === root.window else { return }
        // The pane that held the keyboard is named for this key only.
        heldBeforeKey = nil
        guard root.window?.firstResponder !== root, let chord = KeyChord(event: event), routed.contains(chord.text) else { return }
        // The pane that held the keyboard, so the GPUI view knows where cycling starts.
        let responder = root.window?.firstResponder as? NSView
        heldBeforeKey = panes.first { responder === $0.value.content || responder?.isDescendant(of: $0.value.content) == true }?.key
        _ = root.window?.makeFirstResponder(root)
    }

    /// The item in `menu` whose key equivalent is `key` with `modifiers`.
    static func menuItem(_ menu: NSMenu?, key: String, modifiers: NSEvent.ModifierFlags) -> NSMenuItem? {
        for item in menu?.items ?? [] {
            if let found = menuItem(item.submenu, key: key, modifiers: modifiers) { return found }
            if item.keyEquivalent == key, item.keyEquivalentModifierMask == modifiers { return item }
        }
        return nil
    }

    /// GPUI's view classes whose `performKeyEquivalent:` `keyEquivalentsFollowTheKeyboard` replaced.
    static var keyEquivalentClasses = Set<ObjectIdentifier>()

    /// While a native pane's view inside GPUI's holds the keyboard, a key equivalent goes to the
    /// views under GPUI's, as NSView passes one on, instead of to GPUI. AppKit offers a key carrying Command, Control or
    /// Function (an arrow, ⌘C) to every view in the window before the first responder, and GPUI's
    /// view holds every pane's native view, so GPUI sent an arrow pressed in a web page to its own
    /// text input context, which typed the key's character (0x1C for left) into the page. Every
    /// chord GPUI's keymap runs moves the keyboard to GPUI's view before it lands (`routeKey`), so
    /// none of them is lost.
    static func keyEquivalentsFollowTheKeyboard(_ view: NSView) {
        guard let cls = object_getClass(view), keyEquivalentClasses.insert(ObjectIdentifier(cls)).inserted else { return }
        let sel = #selector(NSView.performKeyEquivalent(with:))
        guard let own = class_getInstanceMethod(cls, sel), let base = class_getInstanceMethod(NSView.self, sel), own != base else { return }
        typealias Imp = @convention(c) (NSView, Selector, NSEvent) -> Bool
        let gpui = unsafeBitCast(method_getImplementation(own), to: Imp.self)
        let nsview = unsafeBitCast(method_getImplementation(base), to: Imp.self)
        let block: @convention(block) (NSView, NSEvent) -> Bool = { view, event in
            // A native pane's view inside GPUI's holds the keyboard: the key is that view's.
            if let held = view.window?.firstResponder as? NSView, held !== view, held.isDescendant(of: view) { return nsview(view, sel, event) }
            return gpui(view, sel, event)
        }
        method_setImplementation(own, imp_implementationWithBlock(block))
    }

    /// The object a nil-targeted action reaches from `window`, as AppKit looks for it: the first
    /// responder and its chain, the window's delegate, the application, its delegate.
    static func target(for action: Selector, from window: NSWindow?) -> NSObject? {
        var responder: NSResponder? = window?.firstResponder
        while let r = responder {
            if r.responds(to: action) { return r }
            responder = r.nextResponder
        }
        let rest: [NSObject?] = [window?.delegate as? NSObject, NSApp, NSApp.delegate as? NSObject]
        return rest.compactMap { $0 }.first { $0.responds(to: action) }
    }

    /// What AppKit asks `target` before it sends `item`'s action: `validateMenuItem:`, else
    /// `validateUserInterfaceItem:`, else enabled.
    static func validates(_ target: NSObject, _ item: NSMenuItem) -> Bool {
        typealias Validate = @convention(c) (NSObject, Selector, NSMenuItem) -> Bool
        for sel in [NSSelectorFromString("validateMenuItem:"), NSSelectorFromString("validateUserInterfaceItem:")] where target.responds(to: sel) {
            guard let imp = class_getMethodImplementation(type(of: target), sel) else { continue }
            return unsafeBitCast(imp, to: Validate.self)(target, sel, item)
        }
        return true
    }

    /// The window is closing: drop every view, the socket and the key monitor.
    func detach() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        for id in Array(panes.keys) { close(id) }
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        pageClient?.stop()
        pageClient = nil
    }

    // MARK: daemon socket (terminal bytes only; layout and ops go over Rust's own socket)
    /// URLSession's default of 1 MiB drops the socket on a larger message (the Beads feed of this repo is 1.3 MB); the Swift app's
    /// `DaemonClient` takes 64 MiB.
    static let maximumMessageSize = 64 << 20
    func connect(_ url: String) {
        guard socket == nil, let target = URL(string: url) else { return }
        socket = URLSession.shared.webSocketTask(with: target)
        socket?.maximumMessageSize = Self.maximumMessageSize
        socket?.resume()
        receive()
    }
    func pluginConnect(_ url: String) {
        pluginURL = url
        guard pluginSocket == nil, let target = URL(string: url) else { return }
        let socket = URLSession.shared.webSocketTask(with: target)
        socket.maximumMessageSize = Self.maximumMessageSize
        pluginSocket = socket
        socket.resume()
        pluginReceive(socket)
    }
    /// Asks wheeld for a pane's script. With no connection, or a send that fails, the pane shows the failure and the request is
    /// not kept: nothing would ever answer it.
    func requestSource(_ host: PaneHost) {
        if pluginSocket == nil, let url = pluginURL { pluginConnect(url) }
        guard let socket = pluginSocket else {
            host.plugin?.fail(message: "no connection to wheeld")
            return
        }
        let req = nextPluginReq
        nextPluginReq += 1
        pluginReqs[req] = host.id
        guard let data = try? JSONSerialization.data(withJSONObject: ["op": "plugin_source", "req": req, "pane": host.id] as [String: Any]),
              let text = String(data: data, encoding: .utf8) else {
            pluginReqs[req] = nil
            host.plugin?.fail(message: "plugin_source could not be encoded")
            return
        }
        Task { @MainActor in
            do { try await socket.send(.string(text)) } catch {
                errors.append("send plugin_source: " + error.localizedDescription)
                guard pluginReqs.removeValue(forKey: req) != nil else { return }
                host.plugin?.fail(message: "could not ask wheeld for the script: " + error.localizedDescription)
            }
        }
    }
    /// `feed_subscribe` or `feed_unsubscribe` for a plugin pane's feed. A subscribe's `error` reply goes to the pane; with no
    /// connection nothing is sent, and the reconnect subscribes every feed the pane holds.
    func sendFeed(_ op: String, pane: String, feed: String, params: JSONValue) {
        guard let socket = pluginSocket else { return }
        let req = nextPluginReq
        nextPluginReq += 1
        if op == "feed_subscribe" { feedReqs[req] = (pane, feed, params) }
        let message: [String: Any] = ["op": op, "req": req, "pane": pane, "feed": feed, "params": params.foundation]
        guard let data = try? JSONSerialization.data(withJSONObject: message), let text = String(data: data, encoding: .utf8) else {
            feedReqs[req] = nil
            return
        }
        Task { @MainActor in
            do { try await socket.send(.string(text)) } catch {
                errors.append("send \(op): " + error.localizedDescription)
                feedReqs[req] = nil
            }
        }
    }
    /// `plugin_action` for a script's `wheel.action`. The reply (`plugin_action` or `error`) goes back to the pane's script;
    /// with no connection the script gets the failure on the next main-queue turn, never from inside its own `update` or `view`.
    func sendAction(pane: String, action: String, args: JSONValue) {
        guard let socket = pluginSocket else {
            panes[pane]?.plugin?.actionFailedLater(action: action, message: "no connection to wheeld")
            return
        }
        let req = nextPluginReq
        nextPluginReq += 1
        actionReqs[req] = (pane, action)
        let message: [String: Any] = ["op": "plugin_action", "req": req, "pane": pane, "action": action, "args": args.foundation]
        guard let data = try? JSONSerialization.data(withJSONObject: message), let text = String(data: data, encoding: .utf8) else {
            actionReqs[req] = nil
            panes[pane]?.plugin?.actionFailedLater(action: action, message: "the action arguments could not be encoded")
            return
        }
        Task { @MainActor in
            do { try await socket.send(.string(text)) } catch {
                errors.append("send plugin_action: " + error.localizedDescription)
                guard let sent = actionReqs.removeValue(forKey: req) else { return }
                panes[sent.pane]?.plugin?.actionFailed(action: sent.action, message: "could not send the action: " + error.localizedDescription)
            }
        }
    }

    func pluginReceive(_ socket: URLSessionWebSocketTask) {
        Task { @MainActor in
            do {
                let message = try await socket.receive()
                let data: Data
                switch message { case .string(let s): data = Data(s.utf8); case .data(let d): data = d; @unknown default: return }
                pluginBackoff = 1
                if let value = try JSONSerialization.jsonObject(with: data) as? [String: Any] { pluginMessage(value) }
                pluginReceive(socket)
            } catch {
                errors.append("plugin socket: " + error.localizedDescription)
                guard pluginSocket === socket else { return }
                pluginLost(error.localizedDescription)
                let wait = pluginBackoff
                pluginBackoff = min(pluginBackoff * 2, 30)
                try? await Task.sleep(nanoseconds: wait * 1_000_000_000)
                // A reconnect of the app's own socket (`plugin_endpoint`) may have opened the next one already.
                guard pluginSocket == nil, panes.values.contains(where: { $0.plugin != nil }), let url = pluginURL else { return }
                pluginResume(url)
            }
        }
    }
    /// The plugin socket is gone: nothing answers the requests that were waiting, and each pane says so until its script loads again.
    func pluginLost(_ reason: String) {
        pluginSocket = nil
        pluginReqs = [:]
        feedReqs = [:]
        let lost = actionReqs
        actionReqs = [:]
        for sent in lost.values { panes[sent.pane]?.plugin?.actionFailed(action: sent.action, message: "lost the connection to wheeld") }
        panes.values.forEach { $0.plugin?.fail(message: "lost the connection to wheeld (\(reason)), retrying") }
    }
    /// A new plugin socket asks for every pane's script again and subscribes every feed again: wheeld forgot them with the old one.
    func pluginResume(_ url: String) {
        pluginConnect(url)
        panes.values.filter { $0.plugin != nil }.forEach(requestSource)
        for (id, host) in panes {
            for (feed, params) in host.plugin?.feeds ?? [] { sendFeed("feed_subscribe", pane: id, feed: feed, params: params) }
        }
    }
    /// The app's own socket reached wheeld again at `url` (wheeld may have restarted, with a new token): the plugin panes it kept
    /// open reconnect there now rather than when the backoff ends, and keep their scripts' state, since a script that loads into
    /// a pane that holds one reloads in place. Their embedded terminals and chats belonged to the sockets `disconnect` dropped;
    /// they attach again when the new pane targets arrive.
    func pluginEndpoint(_ url: String) {
        let moved = url != pluginURL
        pluginURL = url
        guard panes.values.contains(where: { $0.plugin != nil }) else { return }
        for embed in embeds.values { embed.drop() }
        for embed in agentEmbeds.values { embed.drop() }
        if moved, let old = pluginSocket {
            pluginLost("wheeld moved")
            old.cancel(with: .goingAway, reason: nil)
        }
        // wheeld answered: a socket still open here that has not yet heard it died retries after a second, not the backoff.
        pluginBackoff = 1
        guard pluginSocket == nil else { return }
        pluginResume(url)
    }
    /// `plugin_source`, asked for or pushed because the script or one of its package's modules changed, loads into the pane it names; an error answering
    /// a request for one is that pane's failure.
    func pluginMessage(_ value: [String: Any]) {
        switch value["ev"] as? String {
        case "plugin_source":
            guard let pane = value["pane"] as? String, let plugin = panes[pane]?.plugin, let source = value["source"] as? String else { return }
            if let req = value["req"] as? Int { pluginReqs[req] = nil }
            plugin.load(source: source, modules: value["modules"] as? [String: String] ?? [:], chunk: value["chunk"] as? String ?? "pane")
        case "feed_update":
            guard let pane = value["pane"] as? String, let plugin = panes[pane]?.plugin, let feed = value["feed"] as? String else { return }
            if let req = value["req"] as? Int { feedReqs[req] = nil }
            plugin.feedUpdate(feed: feed, params: value["params"], value: value["value"], patch: value["patch"],
                              stale: value["stale"] as? Bool ?? false)
        case "plugin_action":
            guard let req = value["req"] as? Int, let sent = actionReqs.removeValue(forKey: req) else { return }
            panes[sent.pane]?.plugin?.actionResult(action: sent.action, result: value["result"])
        case "error":
            guard let req = value["req"] as? Int else { return }
            if let sent = actionReqs.removeValue(forKey: req) {
                panes[sent.pane]?.plugin?.actionFailed(action: sent.action, message: value["message"] as? String ?? "plugin_action failed")
            } else if let pane = pluginReqs.removeValue(forKey: req) {
                panes[pane]?.plugin?.fail(message: value["message"] as? String ?? "plugin_source failed")
            } else if let sent = feedReqs.removeValue(forKey: req) {
                panes[sent.pane]?.plugin?.feedFailed(feed: sent.feed, params: sent.params.foundation, message: value["message"] as? String ?? "feed_subscribe failed")
            }
        default: break
        }
    }
    func send(_ value: [String: Any]) {
        guard let socket, let data = try? JSONSerialization.data(withJSONObject: value), let string = String(data: data, encoding: .utf8) else { return }
        // URLSession sends from independently spawned tasks may overtake each other:
        // keep key, Return and resize ordering on the wire.
        let previous = sendTask
        sendTask = Task { @MainActor in
            await previous?.value
            sends.append((value["op"] as? String ?? "?") + " " + (value["id"] as? String ?? ""))
            do { try await socket.send(.string(string)) } catch { errors.append("send \(value["op"] as? String ?? "?"): " + error.localizedDescription) }
        }
    }
    func receive() {
        guard let socket else { return }
        let dead = socket
        Task { @MainActor in
            do {
                let message = try await socket.receive()
                let data: Data
                switch message { case .string(let s): data = Data(s.utf8); case .data(let d): data = d; @unknown default: return }
                if let value = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                   value["ev"] as? String == "term_attached", let term = value["id"] as? String {
                    // wheeld answers every `term_attach` with this event and then replays the whole screen as `term_output`
                    // to every session of the term on this connection, not only the new one. The socket is ordered, so a
                    // full reset (ESC c, RIS) now leaves every embed blank and the replay repaints them from one state.
                    let reset = Data([0x1b, 0x63])
                    for embed in embeds.values where embed.termID == term { embed.session?.receive(reset) }
                }
                if let value = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                   value["ev"] as? String == "term_output", let term = value["id"] as? String,
                   let encoded = value["data"] as? String, let bytes = Data(base64Encoded: encoded) {
                    termOutputs += 1
                    for embed in embeds.values where embed.termID == term { embed.bytes += bytes.count; embed.session?.receive(bytes) }
                }
                receive()
            } catch {
                errors.append(error.localizedDescription)
                if self.socket === dead { self.socket = nil }
            }
        }
    }

    // MARK: panes
    func open(_ value: [String: Any]) {
        let id = value["pane"] as! String
        let kind = value["kind"] as! String
        let artifact = (value["artifact"] as? String).map { "\(value["scope"] as? String ?? "")/\($0)" }
        if let existing = panes[id], existing.kind == kind, existing.artifact == artifact, existing.cwd == value["cwd"] as? String, existing.pluginLabel == value["label"] as? String {
            return
        }
        close(id)
        guard let address = value["slot"] as? UInt, let pointer = UnsafeRawPointer(bitPattern: address) else { errors.append("open \(id): no slot"); return }
        let host = PaneHost(id: id, kind: kind, slot: Unmanaged<NSView>.fromOpaque(pointer).takeUnretainedValue())
        host.artifact = artifact
        host.cwd = value["cwd"] as? String
        host.pluginLabel = value["label"] as? String
        if kind == "plugin" {
            let plugin = PluginPane()
            let space = value["space"] as? String
            let ids = (host.pluginLabel ?? "").split(separator: "/", maxSplits: 1).map(String.init)
            plugin.context = [
                "pane": .string(id),
                "space": space.map { .string($0) } ?? .null,
                "cwd": host.cwd.flatMap { $0.isEmpty ? nil : .string($0) } ?? .null,
                "plugin": ids.first.map { .string($0) } ?? .null,
                "plugin_pane": ids.count > 1 ? .string(ids[1]) : .null,
            ]
            plugin.onUpdate = { [weak self, weak plugin] in
                guard let plugin else { return }
                self?.emit(["event": "plugin", "pane": id, "error": plugin.error ?? NSNull()])
            }
            plugin.onFeeds = { [weak self] subscribe, unsubscribe in
                for (feed, params) in unsubscribe { self?.sendFeed("feed_unsubscribe", pane: id, feed: feed, params: params) }
                for (feed, params) in subscribe { self?.sendFeed("feed_subscribe", pane: id, feed: feed, params: params) }
            }
            plugin.onAction = { [weak self] action, args in
                self?.sendAction(pane: id, action: action, args: args)
            }
            let surface = PluginPaneView(pane: plugin)
                .environment(\.paneEmbed, { [weak self] embed in self?.embedView(embed, hostPane: id) })
                .environment(\.paneVideoStream, { [weak self] stream, input in self?.streamView(stream, input: input, hostPane: id) })
            let view = NSHostingView(rootView: AnyView(surface))
            host.plugin = plugin
            host.pluginView = view
            host.slot.addSubview(view)
            panes[id] = host
            emit(["event": "plugin", "pane": id, "error": NSNull()])
            if let ws = value["ws"] as? String { pluginConnect(ws) }
            requestSource(host)
            return
        }
        if kind == "web" {
            let config = WKWebViewConfiguration()
            let artifact = (value["artifact"] as? String).flatMap { name in (value["scope"] as? String).flatMap(ArtifactScope.init(rawValue:)).map { ArtifactRef(scope: $0, name: name) } }
            if let artifact {
                // An artifact's page, as the Swift Mac app opens it (WebPane.swift): its own browser
                // storage, one store per artifact, and its address under the client's scheme, which
                // `PageSchemeHandler` fetches from the daemon with the token in a header.
                let store = WebPanes.store(cwd: value["cwd"] as? String ?? "", artifact: artifact)
                host.store = store
                config.websiteDataStore = WKWebsiteDataStore(forIdentifier: store)
                if let ws = value["ws"] as? String { daemonAddress(ws) }
                let handler = PageSchemeHandler(pane: id, daemon: pageDaemon)
                handler.onDocument = { [weak host] code in host?.status = code }
                config.setURLSchemeHandler(handler, forURLScheme: WebPanes.scheme)
            }
            let web = WKWebView(frame: .zero, configuration: config)
            web.navigationDelegate = self
            host.web = web
            host.slot.addSubview(web)
            if artifact != nil, let page = WebPanes.pageURL(pane: id) {
                host.pageURL = page
                host.url = page.absoluteString
                web.load(URLRequest(url: page))
            }
        }
        panes[id] = host
    }
    /// `render_pane` relayed by the Rust side for a plugin pane: its view's pixels as a PNG,
    /// answered through `emit` with `rendered` or `refused`. The pane host draws a WKWebView pane itself.
    func render(_ value: [String: Any]) {
        let id = value["id"] as! UInt64
        let pane = value["pane"] as! String
        guard let host = panes[pane] else { emit(["answer": "refused", "id": id, "message": RenderRefusal.notShown]); return }
        if let view = host.pluginView, let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
            view.cacheDisplay(in: view.bounds, to: rep)
            // cacheDisplay draws only what AppKit draws (a scroll view's document); SwiftUI content outside one lives in
            // the layer tree alone, so the layers are rendered over it.
            if let layer = view.layer, let context = NSGraphicsContext(bitmapImageRep: rep) {
                let cg = context.cgContext
                cg.saveGState()
                if view.isFlipped { cg.translateBy(x: 0, y: view.bounds.height); cg.scaleBy(x: 1, y: -1) }
                layer.render(in: cg)
                cg.restoreGState()
            }
            // cacheDisplay leaves a WKWebView and a terminal surface blank: each one's own pixels are drawn over its frame.
            Task { @MainActor [weak self] in
                guard let self else { return }
                var parts: [(NSRect, NSImage)] = []
                for web in Self.subviews(of: view, ofType: WKWebView.self) where !web.isHiddenOrHasHiddenAncestor && !web.bounds.isEmpty {
                    if let image = await Self.shot(web) { parts.append((view.convert(web.bounds, from: web), image)) }
                }
                for terminal in Self.subviews(of: view, ofType: DropTerminalView.self) where !terminal.isHiddenOrHasHiddenAncestor && !terminal.bounds.isEmpty {
                    if let image = self.terminalPixels(terminal) { parts.append((view.convert(terminal.bounds, from: terminal), image)) }
                }
                if !parts.isEmpty, let context = NSGraphicsContext(bitmapImageRep: rep) {
                    NSGraphicsContext.saveGraphicsState()
                    NSGraphicsContext.current = context
                    for (rect, image) in parts {
                        // The bitmap's origin is bottom-left; a flipped hosting view's frames count from the top.
                        let y = view.isFlipped ? view.bounds.height - rect.maxY : rect.minY
                        image.draw(in: NSRect(x: rect.minX, y: y, width: rect.width, height: rect.height))
                    }
                    NSGraphicsContext.restoreGraphicsState()
                }
                guard let png = rep.representation(using: .png, properties: [:]) else { self.emit(["answer": "refused", "id": id, "message": "the page could not be encoded as a PNG"]); return }
                self.emit(["answer": "rendered", "id": id, "png": png.base64EncodedString()])
            }
        } else {
            emit(["answer": "refused", "id": id, "message": "the GPUI client draws this pane itself; its picture is not served yet"])
        }
    }
    /// Every view of type `T` in `view`'s subview tree.
    static func subviews<T: NSView>(of view: NSView, ofType: T.Type) -> [T] {
        view.subviews.flatMap { ($0 as? T).map { [$0] } ?? subviews(of: $0, ofType: T.self) }
    }
    /// The part of `web` in view as WebKit draws it, or nil when it drew nothing.
    static func shot(_ web: WKWebView) async -> NSImage? {
        await withCheckedContinuation { continuation in web.takeSnapshot(with: nil) { image, _ in continuation.resume(returning: image) } }
    }
    /// `pane_reload`: a plugin pane runs its script again from the top, and an artifact page opens its address again
    /// rather than `WKWebView.reload()`, which does nothing when the first load never committed (the daemon was down).
    func reload(_ id: String) {
        guard let host = panes[id] else { return }
        if let plugin = host.plugin {
            host.reloads += 1
            plugin.restart()
            requestSource(host)
            return
        }
        guard let page = host.pageURL else { return }
        host.reloads += 1
        host.web?.load(URLRequest(url: page))
    }
    /// The daemon's address and token for the scheme handler, from the socket address a pane was given.
    func daemonAddress(_ ws: String) {
        guard var parts = URLComponents(string: ws), let token = parts.queryItems?.first(where: { $0.name == "token" })?.value else { return }
        parts.queryItems = nil
        pageDaemon.url = parts.url
        pageDaemon.token = token
    }
    /// What a plugin page's embedded pane draws as: the pane's terminal, or its agent's conversation.
    func embedView(_ embed: PaneEmbed, hostPane: String) -> AnyView? {
        switch embed.kind {
        case .terminal: return AnyView(EmbeddedTerminal(native: self, hostPane: hostPane, pane: embed.pane))
        case .agentChat: return AnyView(EmbeddedAgentChat(native: self, hostPane: hostPane, pane: embed.pane))
        case .artifact:
            guard let ws = pluginURL, pages(ws) != nil else { return nil }
            daemonAddress(ws)
            // Through `pageClient`, not the client at hand: `disconnect` replaces it, and a held one would swallow every send.
            let embeds = artifactEmbeds ?? ArtifactEmbeds(daemon: pageDaemon, send: { [weak self] in self?.pageClient?.send($0) },
                                                          nextReq: { [weak self] in self?.pageClient?.req() ?? 0 })
            artifactEmbeds = embeds
            var cwd = ""
            if case let .string(dir)? = panes[hostPane]?.plugin?.context["cwd"] { cwd = dir }
            return embeds.view(pane: hostPane, name: embed.pane, cwd: cwd)
        }
    }
    /// Video stream `stream` inside plugin pane `hostPane`'s page (ADR-0053), asked for over the page connection.
    func streamView(_ stream: String, input: Bool, hostPane: String) -> AnyView? {
        guard let ws = pluginURL, pages(ws) != nil else { return nil }
        // Through `pageClient`, not the client at hand: `disconnect` replaces it, and a held one would swallow every request.
        let streams = videoStreams ?? VideoStreams(send: { [weak self] in self?.pageClient?.send($0) }, nextReq: { [weak self] in self?.pageClient?.req() ?? 0 })
        videoStreams = streams
        return streams.view(pane: hostPane, stream: stream, input: input)
    }
    /// `agent`'s chat, attached over the page connection the first time an embed asks for it.
    func agentChat(_ agent: String) -> AgentChat {
        chatHolds[agent, default: 0] += 1
        if let held = chats[agent] { return held }
        let chat = AgentChat(name: agent)
        chat.sendUnsend = { [weak self] turn in self?.agentRequest(agent) { .agentUnsend(req: $0, agent: agent, turn: turn) } }
        chat.sendNow = { [weak self] turn in self?.agentRequest(agent) { .agentSendNow(req: $0, agent: agent, turn: turn) } }
        chat.sendClearQueue = { [weak self] in self?.agentRequest(agent) { .agentClearQueue(req: $0, agent: agent) } }
        chats[agent] = chat
        if pageClient?.connection == .connected { attachAgent(agent) } else if let ws = pluginURL { _ = pages(ws) }
        return chat
    }
    /// Stream `agent`'s conversation; `agent_attached` replaces the transcript. A connection that is not up yet attaches
    /// every chat when it comes up (`pages`).
    func attachAgent(_ agent: String) {
        guard let ws = pluginURL else { return }
        _ = pages(ws)
        if let req = agentRequest(agent, { .agentAttach(req: $0, agent: agent) }) { attachReqs.insert(req) }
    }
    /// wheeld has just said something about `agent`: when its attach was refused, the agent is there now.
    func attachAgainIfRefused(_ agent: String) {
        guard refusedAttach.remove(agent) != nil, chats[agent] != nil else { return }
        attachAgent(agent)
    }
    /// Send `make`'s message with a new req, remembered so an error reply reaches `agent`'s chat. A connection that is
    /// down sends nothing: the chat says so, and the connection attaches again when it comes back.
    @discardableResult func agentRequest(_ agent: String, _ make: (UInt64) -> ClientMsg) -> UInt64? {
        guard let client = pageClient, client.connection == .connected else {
            chats[agent]?.error = "Not connected to wheeld."
            return nil
        }
        let req = client.req()
        agentReqs[req] = agent
        client.send(make(req))
        return req
    }
    /// One embed let go of `agent`: the last one detaches it.
    func releaseChat(_ agent: String) {
        let left = (chatHolds[agent] ?? 1) - 1
        guard left <= 0 else { chatHolds[agent] = left; return }
        chatHolds[agent] = nil
        chats[agent] = nil
        refusedAttach.remove(agent)
        pageClient?.send(.agentDetach(agent: agent))
    }
    /// Send the chat's draft; it stays in the box when the connection is down.
    func sendAgent(_ chat: AgentChat) {
        let text = chat.draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        guard pageClient?.connection == .connected else {
            chat.error = "Not connected to wheeld; your message is still in the box."
            return
        }
        chat.error = nil
        chat.pinned = true
        agentRequest(chat.name) { .agentSend(req: $0, agent: chat.name, text: text) }
        chat.draft = ""
    }
    /// Allow or deny a waiting request; the card goes when wheeld broadcasts the answer.
    func answer(_ perm: PermInfo, _ choice: String) {
        agentRequest(perm.agent) { .permAnswer(req: $0, id: perm.id, choice: choice) }
    }
    var taskOutputAsks: [UInt64: (agent: String, task: String)] = [:]
    func readTaskOutput(_ chat: AgentChat, task: String) {
        guard let client = pageClient, client.connection == .connected else { return }
        let req = client.req()
        taskOutputAsks[req] = (chat.name, task)
        client.send(.taskOutput(req: req, agent: chat.name, task: task))
    }
    /// What the page connection hears about the embedded agents and the requests they wait on.
    func agentMessage(_ msg: ServerMsg) {
        switch msg {
        case let .status(_, _, _, _, _, perms, _, _, _, _, _, _, _, _):
            embedPerms.list = perms
        case let .agentAttached(req, agent, state, events, config, context, now):
            if let req { agentReqs[req] = nil; attachReqs.remove(req) }
            refusedAttach.remove(agent)
            guard let chat = chats[agent] else { return }
            chat.state = state
            chat.reset(events, daemonNow: now)
            chat.setConfig(config)
            chat.context = context
            chat.error = nil
            chat.notice = nil
            chat.title = nil
        case let .agent(agent, event): chats[agent]?.apply(event)
        case let .agentStateChanged(agent, state):
            chats[agent]?.state = state
            attachAgainIfRefused(agent)
        case let .agentConfig(agent, options):
            attachAgainIfRefused(agent)
            chats[agent]?.setConfig(options)
        case let .agentContext(agent, context): chats[agent]?.context = context
        case let .agentFooter(agent, footer): chats[agent]?.footer = footer
        case let .agentCommands(agent, commands): chats[agent]?.commands = Set(commands)
        case let .agentTitle(agent, title): chats[agent]?.title = title
        case let .agentNotice(agent, notice): chats[agent]?.notice = notice
        case let .agentQueued(req, _, _): agentReqs[req] = nil
        case let .taskOutput(req, text, truncated):
            // A reply for a task that ended meanwhile is not kept.
            if let ask = taskOutputAsks.removeValue(forKey: req), let chat = chats[ask.agent], chat.footer?.tasks.contains(where: { $0.id == ask.task }) == true {
                chat.taskOutputs[ask.task] = (text, truncated)
            }
        case let .permRequested(perm):
            if !embedPerms.list.contains(where: { $0.id == perm.id }) { embedPerms.list.append(perm) }
        case let .permResolved(id, _):
            embedPerms.list.removeAll { $0.id == id }
        case let .error(req, _, message):
            guard let req else { break }
            taskOutputAsks[req] = nil
            if let agent = agentReqs.removeValue(forKey: req) {
                chats[agent]?.error = message
                if attachReqs.remove(req) != nil { refusedAttach.insert(agent) }
            }
        default: break
        }
    }
    /// `term` is no longer shown by `except`: detach it when no other embed shows it.
    func release(_ term: String, except: EmbedTerminal? = nil) {
        guard !embeds.values.contains(where: { $0 !== except && $0.termID == term }) else { return }
        send(["op": "term_detach", "id": term])
    }
    func close(_ id: String) {
        guard let host = panes.removeValue(forKey: id) else { return }
        for (key, embed) in embeds where embed.hostPane == id { embeds[key] = nil; embed.drop() }
        let gone = agentEmbeds.filter { $0.value.hostPane == id }
        for key in gone.keys { agentEmbeds[key] = nil }
        for embed in gone.values { embed.drop() }
        if let web = host.web { PageSchemeHandler.close(web) }
        host.slot.subviews.forEach { $0.removeFromSuperview() }
        if let plugin = host.plugin {
            plugin.close()
            pluginReqs = pluginReqs.filter { $0.value != id }
            feedReqs = feedReqs.filter { $0.value.pane != id }
            if !panes.values.contains(where: { $0.plugin != nil }) {
                let socket = pluginSocket
                pluginSocket = nil
                pluginReqs = [:]
                feedReqs = [:]
                socket?.cancel(with: .goingAway, reason: nil)
            }
        }
    }
    /// The page connection, opened on first use from the socket address the pane was given.
    func pages(_ ws: String) -> DaemonClient? {
        if let pageClient { return pageClient }
        guard var parts = URLComponents(string: ws), let token = parts.queryItems?.first(where: { $0.name == "token" })?.value else { return nil }
        parts.queryItems = nil
        guard let url = parts.url else { return nil }
        let client = DaemonClient(endpoint: DaemonEndpoint(url: url, token: { token }))
        client.onConnect = { [weak self] status in
            guard let self else { return }
            // A fresh connection holds no attach: every embedded agent attaches again, and the status names the waiting requests.
            agentReqs = [:]; attachReqs = []; refusedAttach = []; taskOutputAsks = [:]
            agentMessage(status)
            for (agent, chat) in chats { chat.error = nil; attachAgent(agent) }
            artifactEmbeds?.connected()
            videoStreams?.connected()
        }
        client.onMessage = { [weak self] message in
            guard let self else { return }
            agentMessage(message)
            artifactEmbeds?.handle(message)
            videoStreams?.handle(message)
        }
        client.log = { [weak self] in self?.errors.append("pages: " + $0) }
        client.start()
        pageClient = client
        return client
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard let host = panes.values.first(where: { $0.web === webView }) else { return }
        host.loads.append(webView.title ?? "")
        webView.evaluateJavaScript("document.body ? document.body.innerText : ''") { [weak host] result, _ in
            MainActor.assumeIsolated { host?.text = result as? String }
        }
    }
    /// An artifact page stays inside its pane's folder on the daemon; a link anywhere else opens in
    /// the default browser (WebPane.swift's rules).
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let host = panes.values.first(where: { $0.web === webView }), let page = host.pageURL else { return .allow }
        guard let target = action.request.url else { return .cancel }
        if WebPanes.allowsInlineFrame(target, isMainFrame: action.targetFrame?.isMainFrame ?? true) { return .allow }
        let path = URLComponents(url: target, resolvingAgainstBaseURL: false)?.path ?? target.path
        if target.scheme == page.scheme, target.host == page.host, target.port == page.port, path.hasPrefix("/artifacts/\(host.id)/") { return .allow }
        if action.navigationType == .linkActivated { NSWorkspace.shared.open(target) }
        return .cancel
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: any Error) { errors.append(error.localizedDescription) }

    // MARK: pixels
    // Ghostty replaces its initial CAMetalLayer with an IOSurfaceLayer. Read the real renderer
    // surface currently attached to the view, never redraw text.
    func terminalPixels(_ terminal: AppTerminalView) -> NSImage? {
        guard let contents = terminal.layer?.contents, CFGetTypeID(contents as CFTypeRef) == IOSurfaceGetTypeID() else { return nil }
        let surface = unsafeBitCast(contents as AnyObject, to: IOSurfaceRef.self)
        guard IOSurfaceLock(surface, [], nil) == 0 else { return nil }
        defer { IOSurfaceUnlock(surface, [], nil) }
        let width = IOSurfaceGetWidth(surface), height = IOSurfaceGetHeight(surface), row = IOSurfaceGetBytesPerRow(surface)
        let data = Data(bytes: IOSurfaceGetBaseAddress(surface), count: row * height)
        guard let provider = CGDataProvider(data: data as CFData),
              let cg = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: row,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue),
                provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent) else { return nil }
        return NSImage(cgImage: cg, size: CGSize(width: width, height: height))
    }

    // MARK: commands
    func command(_ value: [String: Any]) -> [String: Any] {
        switch value["op"] as? String {
        case "open": open(value)
        case "close": close(value["pane"] as! String)
        case "reload": reload(value["pane"] as! String)
        case "render": render(value)
        case "pane_targets":
            var targets: [String: [String: String]] = [:]
            for (pane, held) in value["panes"] as? [String: [String: String]] ?? [:] { targets[pane] = held }
            paneTargets = targets
            targetsKnown = true
            for embed in embeds.values { embed.resolve() }
            for embed in agentEmbeds.values { embed.resolve() }
        case "plugin":
            // A plugin pane's state, or one of its controls used as a pointer would: `press` a component, `set` a control's value
            // to any JSON value, `raise` the action at `key` of a component (a Table's `rowAction`, a Canvas hit's `hits.0.action`) with `context`,
            // `agent_send` the text `value` through the composer of the agent chat embedding pane `component`, `stream_key` type the
            // characters `value` on the VideoStream of stream `component` as its view takes a key, `stream_click` click its middle,
            // `hover` hold a pointer on `component` (no component takes it away; the client's hover state, with no call to the script).
            // The reply's `hover.lit` is what the last draw lit; the held node shows in it from the next `state`, after SwiftUI draws.
            guard let pane = value["pane"] as? String, let plugin = panes[pane]?.plugin else { return ["ok": false, "message": "no plugin pane"] }
            var ok = true
            switch value["action"] as? String {
            case "press": ok = (value["component"] as? String).map { plugin.press(component: $0) } ?? false
            case "hover": ok = plugin.hover(component: value["component"] as? String)
            case "set":
                ok = (value["component"] as? String).flatMap { c in value["value"].map { plugin.set(component: c, value: $0) } } ?? false
            case "raise":
                ok = (value["component"] as? String).flatMap { c in (value["key"] as? String).map { plugin.raise(component: c, key: $0, context: value["context"]) } } ?? false
            case "scroll":
                if plugin.fillsPane { ok = false; break }
                // The pane's ScrollView moves without the pointer: `to` is `top` or `bottom`, `dy` a distance in points (positive is down).
                if let dy = value["dy"] as? Double { plugin.scroll(.by(dy)) }
                else if let to = value["to"] as? String, to == "top" || to == "bottom" { plugin.scroll(to == "top" ? .top : .bottom) }
                else { ok = false }
            case "agent_send":
                // The embedded agent chat of pane `component` sends `value` as its composer's Send would.
                let embed = agentEmbeds.values.first { $0.hostPane == pane && $0.pane == value["component"] as? String }
                if let chat = embed?.chat, let text = value["value"] as? String { chat.draft = text; sendAgent(chat) } else { ok = false }
            case "stream_key":
                ok = (value["component"] as? String).flatMap { s in (value["value"] as? String).map { videoStreams?.type(pane: pane, stream: s, characters: $0) ?? false } } ?? false
            case "stream_click":
                ok = (value["component"] as? String).map { videoStreams?.click(pane: pane, stream: $0) ?? false } ?? false
            default: break
            }
            return ["ok": ok, "state": plugin.stateJSON, "error": plugin.error ?? NSNull(), "reloads": plugin.reloads, "restarts": plugin.restarts, "components": plugin.componentTexts, "feeds": plugin.feedJSON,
                "fills": plugin.fillsPane, "hover": ["held": plugin.tree.hover.held ?? NSNull(), "lit": plugin.tree.hover.lit.sorted()] as [String: Any],
                "perf": ["update_ms": plugin.perf.updateMs, "view_ms": plugin.perf.viewMs, "reconcile_ms": plugin.perf.reconcileMs,
                         "nodes": plugin.perf.nodes, "renders": plugin.perf.renders, "bodies": ViewPerf.bodies, "cells": ViewPerf.cells] as [String: Any],
                "scroll": (plugin.fillsPane ? nil : plugin.scrollMetrics).map { ["offset": $0.offset, "content": $0.content, "visible": $0.visible] as [String: Any] } ?? NSNull(),
                "embeds": embeds.values.filter { $0.hostPane == pane }.map { ["pane": $0.pane, "term": $0.termID ?? NSNull(), "bytes": $0.bytes] as [String: Any] }
                    + agentEmbeds.values.filter { $0.hostPane == pane }.map { embed in
                        ["pane": embed.pane, "agent": embed.chat?.name ?? NSNull(), "note": embed.note, "state": embed.chat?.state.map { "\($0)" } ?? NSNull(),
                         "items": embed.chat?.displayItems.count ?? 0, "draft": embed.chat?.draft ?? "", "error": embed.chat?.error ?? NSNull()] as [String: Any] }
                    + (artifactEmbeds?.state(pane: pane) ?? []) + (videoStreams?.state(pane: pane) ?? [])]
        case "plugin_endpoint":
            if let ws = value["ws"] as? String { pluginEndpoint(ws) }
        case "disconnect":
            // Sends queued behind a socket that is gone never finish; the next socket starts a new chain.
            socket?.cancel(with: .goingAway, reason: nil)
            socket = nil
            sendTask = nil
            pageClient?.stop()
            pageClient = nil
            embedPerms.list = []
            agentReqs = [:]; attachReqs = []; taskOutputAsks = [:]
        case "keymap":
            routed = Set((value["chords"] as? [String]) ?? [])
        case "post_key":
            // Real NSEvents to NSWindow.sendEvent: first-responder routing and the views' own key
            // handling run as for hardware events; nothing touches the OS keyboard or focus.
            let text = value["text"] as! String
            let code = UInt16(value["code"] as? Int ?? 0)
            var flags: NSEvent.ModifierFlags = []
            if value["command"] as? Bool == true { flags.insert(.command) }
            if value["shift"] as? Bool == true { flags.insert(.shift) }
            if value["control"] as? Bool == true { flags.insert(.control) }
            if value["option"] as? Bool == true { flags.insert(.option) }
            // An arrow, Home, End or F-key carries these two flags from a hardware keyboard.
            if value["function"] as? Bool == true { flags.formUnion([.function, .numericPad]) }
            var claimed: String?
            for eventType in [NSEvent.EventType.keyDown, .keyUp] {
                if let event = NSEvent.keyEvent(with: eventType, location: .zero, modifierFlags: flags,
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: root.window?.windowNumber ?? 0,
                    context: nil, characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: code) {
                    if eventType == .keyDown {
                        routeKey(event)
                        // NSApplication offers a key down carrying Command, Control or Function to the
                        // window's views as a key equivalent before the first responder gets it.
                        if !flags.isDisjoint(with: [.command, .control, .function]), root.window?.performKeyEquivalent(with: event) == true {
                            claimed = root.window?.firstResponder.map { String(describing: type(of: $0)) }
                            continue
                        }
                    }
                    root.window?.sendEvent(event)
                }
            }
            // The first responder's class when a view claimed the key down as a key equivalent.
            return ["claimed_by": claimed as Any? ?? NSNull()]
        case "menu_key":
            // The main menu's item for a key equivalent, sent as AppKit sends it from the key window:
            // validated, from the first responder. A hidden window is never key, so the menu's own
            // dispatch (which WebKit hands a key it does not use) finds no target there.
            let key = value["key"] as! String
            guard let item = Self.menuItem(NSApp.mainMenu, key: key, modifiers: .command), let action = item.action else { return ["sent": false, "item": NSNull()] }
            guard let target = Self.target(for: action, from: root.window), Self.validates(target, item) else { return ["sent": false, "item": item.title] }
            return ["sent": NSApp.sendAction(action, to: target, from: item), "item": item.title, "target": String(describing: type(of: target))]
        default: break
        }
        return status()
    }

    func status() -> [String: Any] {
        var out: [String: Any] = ["pid": ProcessInfo.processInfo.processIdentifier, "errors": errors,
            "root_focused": root.window?.firstResponder === root, "held_before_key": heldBeforeKey ?? "",
            "first_responder": root.window?.firstResponder.map { String(describing: type(of: $0)) } ?? "nil",
            "key_window": root.window?.isKeyWindow ?? false, "visible": root.window?.isVisible ?? false,
            "app_active": NSApp.isActive, "native_subviews": root.subviews.count,
            "socket": socket.map { "state \($0.state.rawValue) code \($0.closeCode.rawValue)" } ?? "none", "term_outputs": termOutputs, "sends": sends]
        var panesOut: [String: Any] = [:]
        for (id, host) in panes {
            // Where the slot sits and what is cut out of it the Rust pane host adds.
            var entry: [String: Any] = ["kind": host.kind,
                "focused": root.window?.firstResponder.map { $0 === host.content || (($0 as? NSView)?.isDescendant(of: host.content) ?? false) } ?? false,
                "view_identity": String(describing: Unmanaged.passUnretained(host.content).toOpaque())]
            if host.kind == "web" {
                entry["loads"] = host.loads; entry["url"] = host.url ?? ""; entry["reloads"] = host.reloads
                entry["status"] = host.status ?? NSNull(); entry["store"] = host.store?.uuidString ?? ""; entry["text"] = host.text ?? ""
            } else if let p = host.plugin {
                entry["plugin"] = ["state": p.stateJSON, "error": p.error ?? NSNull(), "reloads": p.reloads, "restarts": p.restarts, "chunk": p.chunk, "components": p.componentTexts, "feeds": p.feedJSON]
            }
            panesOut[id] = entry
        }
        out["panes"] = panesOut
        return out
    }
}

/// One Native per GPUI window, by the id `attach` hands back.
@MainActor private var natives: [Int32: Native] = [:]
@MainActor private var nextWindow: Int32 = 1
@_cdecl("wheel_native_attach") @MainActor
public func attach(_ pointer: UnsafeMutableRawPointer, _ hidden: Int32, _ emit: @escaping Emit) -> Int32 {
    let id = nextWindow
    nextWindow += 1
    let native = Native(root: Unmanaged<NSView>.fromOpaque(pointer).takeUnretainedValue(), window: id, hidden: hidden != 0, emit: emit)
    natives[id] = native
    return id
}
@_cdecl("wheel_native_command") @MainActor
public func command(_ pointer: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar> {
    do {
        let value = try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as! [String: Any]
        let window = Int32(value["window"] as? Int ?? 0)
        var result: [String: Any]
        if value["op"] as? String == "detach" {
            natives.removeValue(forKey: window)?.detach()
            result = ["detached": true]
        } else {
            // GPUI may render before the bridge attaches to the window.
            result = natives[window]?.command(value) ?? ["detached": true]
        }
        let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        return strdup(String(decoding: data, as: UTF8.self))!
    } catch { return strdup("{\"error\":\"JSON bridge failed\"}")! }
}
@_cdecl("wheel_native_free")
public func releaseString(_ pointer: UnsafeMutableRawPointer) { free(pointer) }
