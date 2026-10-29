import AppKit
import Foundation
import GhosttyTerminal
import SwiftUI
import WebKit
import WheelKit
import WheelUI

/// Everything the window shows, fed by one connection to wheeld. The daemon holds the real
/// state (ADR-0001): the spaces and their split trees arrive whole in each `spaces` broadcast,
/// and this model changes them only by sending ops. Quitting and relaunching loses nothing.
/// The release daemon's address, for a build that carries no wheeld of its own.
let wheelDefaultListen = "127.0.0.1:7420"

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var connection: DaemonClient.Connection = .connecting
    @Published private(set) var spaces: [SpaceInfo] = []
    /// The ranks `status.dots` lists: the statuses that draw a dot. Every rank until the daemon says.
    @Published private(set) var statusDots = Set(StatusRank.allCases)
    /// The settings list asked at connect for `status.dots`, so the dots follow the setting with
    /// the Settings window closed.
    private var statusDotsReq: UInt64?
    /// How each space's chats draw, from its `chat.*` settings; a chat reads its own space's entry.
    @Published private(set) var appearances = ChatAppearances()
    /// The outstanding settings lists asked for `appearances`, by request.
    private var appearanceReqs: [UInt64: String] = [:]
    /// The last layout line logged, and each terminal's last surface line: the same line again says
    /// nothing new.
    private var lastLayoutLog = ""
    private var lastSurfaceLog: [String: String] = [:]
    /// The space the daemon last reported as its active one: what a CLI command with no space
    /// acts on. Each window shows its own space; this follows the focused window's.
    private(set) var daemonActive: String?
    /// The daemon's active space before the latest report, so a change something else made shows.
    var lastDaemonActive: String?
    /// The last layout op the daemon refused, shown above the grid until the next op that
    /// changes the layout (a focus click leaves it up).
    @Published private(set) var layoutError: String?
    /// Which window holds each space, and which spaces run with none (ADR-0014). Kept in this
    /// app's preferences; the daemon holds no part of it. Written only by `editWindows` and `editLayoutSilently`.
    @Published private(set) var layout = WindowLayout.saved()
    /// Apply `change` to the layout, then save it and bring the windows and surfaces in line
    /// (`commitWindows`). `commit` gets `change`'s result and whether the layout changed, and says
    /// whether to commit; the default always does.
    @discardableResult
    func editWindows<R>(commit: (R, _ changed: Bool) -> Bool = { _, _ in true }, _ change: (inout WindowLayout) -> R) -> R {
        let before = layout
        let result = change(&layout)
        if commit(result, layout != before) { commitWindows() }
        return result
    }

    /// Apply `change` and save the layout without touching the windows or surfaces: only the
    /// focused window or a sidebar moved.
    func editLayoutSilently(_ change: (inout WindowLayout) -> Void) {
        change(&layout)
        layout.save()
        objectWillChange.send()
    }

    /// Windows are opened and closed by this; set by the app delegate.
    weak var windowHost: WindowHost?
    /// The app is quitting: its windows close without sending their spaces to the background.
    var quitting = false
    /// The first space list since launch has been matched to the saved windows.
    var reconciled = false
    /// Ratios of dividers being dragged, drawn in place of the tree's. A divider still under the
    /// pointer keeps its entry through any broadcast; once its `split_ratio` is sent, the entry
    /// goes with the next broadcast (sent even when the value is unchanged) or a refusal.
    private(set) var dragRatios: [String: Double] {
        get { gridDrag.ratios }
        set { gridDrag.ratios = newValue }
    }
    /// The drag state the grid draws, held apart from the model so a pointer move during a drag
    /// redraws the grid and its highlights, not every pane's view.
    let gridDrag = GridDragState()
    /// Dividers whose drag ended and whose `split_ratio` went out, waiting for the daemon.
    private var settling: Set<String> = []
    /// A pane being dragged by its header. The grid keeps every pane in its slot and highlights
    /// where the drop would land; the daemon's tree changes only when the drop is sent, and
    /// `committed` holds the drag until the daemon's answer arrives.
    struct PaneDrag: Equatable {
        let pane: String
        /// A held mouse button drives it; the debug endpoint's drags have none.
        var byPointer = false
        /// Where it docks in its own space.
        var drop: PaneDrop?
        /// Where it goes when the pointer has left its space (another space's pane, a tab, a row, a
        /// tab strip or the desktop); never set together with `drop`.
        var away: PaneAway?
        var committed = false
    }
    /// The sidebar, tabs and menu bar outline a drop target through `paneIsOver`, so the model
    /// publishes when `away` changes; a pointer move over a dock target does not touch the panes.
    var paneDrag: PaneDrag? {
        get { gridDrag.pane }
        set {
            if gridDrag.pane?.away != newValue?.away { objectWillChange.send() }
            gridDrag.pane = newValue
        }
    }
    /// Where each window's grid is drawn, in its window's top-left space: a pane dragged out of its
    /// space is hit-tested against the space that window shows.
    var gridFrames: [String: CGRect] = [:]
    /// A pane dragged over another space's tab for long enough shows that space; this is the pending wait.
    var paneHoldTask: Task<Void, Never>?
    /// The space the wait above is for.
    var paneHoldSpace: String?
    /// What each window showed before a held pane opened another space, to put back on a cancel.
    var paneDragShown: [String: String] = [:]
    /// The new space the daemon is about to report for a `pane_move`, and where the window layout puts it.
    var pendingNewSpace: PendingNewSpace?
    /// Each space's panes as the daemon's tree places them, which a drag is hit-tested against so the
    /// preview moving panes aside never moves the target under the pointer. Not published.
    var baseLayouts: [String: WheelKit.GridLayout] = [:]
    /// The pane each space shows alone, by space id. Client-only: the daemon's tree is untouched, and
    /// an entry whose pane has left its space is ignored by `zoomedPane(in:)`.
    @Published private(set) var zoomedPanes: [String: String] = [:]
    var dragMonitor: Any?
    /// Escape cancelled the drag in progress; the gesture's remaining events and its end do nothing.
    var dragCancelled = false
    /// Every attached session by name: one per agent pane in the layout.
    @Published private(set) var chats: [String: SessionChat] = [:]
    /// Permission requests waiting on a person, from every session.
    @Published private(set) var perms: [PermInfo] = []
    /// Every agent the daemon knows, with whether its program was found.
    @Published private(set) var agents: [AgentInfo] = [] {
        // An agent found or lost adds or removes its "Start … in This Pane" action.
        didSet { if agents != oldValue { rebuildKeymap() } }
    }
    /// Which key runs which action: the built-in keys with the `keybind` setting applied over them.
    /// Written only by `rebuildKeymap`.
    @Published var keymap = Keymap(defaults: [])
    /// The `keybind` setting as wheeld last reported it.
    var keybindSetting = ""
    /// The shortcut sheet (⌘/) is showing over the focused window.
    @Published var shortcutsOpen = false
    /// A destructive step a key, a menu or a button asked for, waiting on the person's yes in the
    /// window it was asked in.
    @Published var confirmation: Confirmation?
    /// The main window whose close button asked what to do with its spaces; it shows the sheet.
    @Published var closingWindow: String?
    /// The Settings window: wheel's settings for the chosen project, and the edits to them.
    let settings = SettingsModel()
    /// Settings › Artifacts: the built-in artifacts and what is installed.
    let builtinsModel = BuiltinsModel()
    /// The usage strip and the Usage window: every coding agent's usage, from the daemon.
    let usage = UsageModel()
    private var usageTimer: Timer?
    /// The artifact palette open inside a pane, by pane id: at most one per pane, drawn over that
    /// pane alone.
    @Published var palettes: [String: PanePalette] = [:]
    /// The panes whose context meter shows the raw figures instead of the percent. A pane starts
    /// on the percent and returns to it when it closes.
    @Published var rawContextPanes: Set<String> = []
    /// `artifact_ls` requests, by request id, so the answer reaches the palette that asked.
    var paletteReqs: [UInt64: String] = [:]
    /// The command palette over the window: nil while closed. Its recent directories come from the
    /// daemon each time it opens.
    @Published var commandPalette: CommandPalette?
    @Published var recentDirs: [RecentDir] = []
    /// The `space_recent` request still unanswered, so a stale answer is ignored.
    var recentReq: UInt64?
    /// The `dir_list` requests still unanswered, each with the folder it asked about.
    var dirReqs: [UInt64: String] = [:]
    /// The `dir_roots` request still unanswered.
    var rootsReq: UInt64?
    /// The `[runner]` commands of the space the command palette opened over, one "Run" row each.
    @Published var runners: [RunnerInfo] = []
    /// The `runner_ls` request still unanswered.
    var runnersReq: UInt64?
    /// The artifacts the command palette lists, from the `artifact_ls` request `paletteArtifactsReq`.
    @Published var paletteArtifacts: [ArtifactInfo] = []
    var paletteArtifactsReq: UInt64?
    /// `pane_split` requests the command palette made for an artifact, by request id: the pane the
    /// split creates then shows that artifact.
    var splitArtifacts: [UInt64: ArtifactInfo] = [:]
    /// The popover row the pointer is over, whose place buttons show.
    @Published var popoverHover: String?
    /// The pairing window's state, the clipboard clear waiting out its minute, and the window itself.
    @Published var pairing = PairingState()
    var copyPending = false
    /// The token the clipboard holds, until its minute is up.
    var copiedToken: String?
    /// The tokens the open pairing window showed.
    var windowTokens: [String] = []
    /// Tokens to cancel once the daemon is reachable.
    var cancelsToSend: [String] = []
    var clipboardClear: Task<Void, Never>?
    var pairingRefresh: Timer?
    /// Asks after the log shipping state while the Settings window is open.
    private var logShippingRefresh: Timer?
    var pairingPanel: PairingPanel?
    /// The server was stopped from the menu bar item: the app does not start it again at launch.
    @Published var serverStoppedByMenu = Prefs.store.bool(forKey: "serverStopped")
    /// The header's details are open, and its Stop Server confirmation is showing.
    @Published var serverExpanded = false
    @Published var serverConfirming = false
    /// Why the last stop or start did not work.
    @Published var serverProblem: String?
    /// When the connected server started, from the uptime in its status.
    private(set) var serverStarted: Date?
    var serverControl: ServerControl?
    /// A space being dragged by its row or title chip, and where releasing it would put it.
    @Published var spaceDrag: SpaceDrag?
    /// Where each sidebar row, sidebar and popover card is drawn, in its window's top-left space:
    /// what a drag is hit-tested against. Not published, so measuring redraws nothing.
    var rowFrames: [String: [String: CGRect]] = [:]
    var sidebarFrames: [String: CGRect] = [:]
    /// Each window's tab strip, in its window's top-left space; its tabs are in `rowFrames`.
    var tabStripFrames: [String: CGRect] = [:]
    /// When the press that is about to switch a space began (`ProcessInfo.systemUptime`), and the
    /// switch it started once the click lands, until the window layout that shows it is drawn.
    var spacePressAt: TimeInterval?
    var pendingSwitch: (space: String, pressAt: TimeInterval)?
    var cardFrames: [String: CGRect] = [:]
    /// Each popover row, by its card's key (a window id or `MenuBarWindow.backgroundKey`) and space id, in the popover's top-left space.
    var popoverRowFrames: [String: [String: CGRect]] = [:]
    /// The rows' places when the drag began: the gap moving rows aside never moves the target under the pointer.
    var frozenRowFrames: [String: [String: CGRect]]?
    var frozenSidebarFrames: [String: CGRect]?
    var frozenTabStripFrames: [String: CGRect]?
    var dragMonitorForSpaces: Any?
    var desktopOverlay: DesktopDropOverlay?
    var dragChip: DragChipWindow?
    /// Where the dragged pane's header was grabbed, from its top left corner.
    var paneGrab: CGSize?
    /// Where the drag's preview card was when it was released: the little window it opens grows from there.
    var littleOrigin: CGRect?
    /// The space whose name is being edited in place (its sidebar row or tab), and the window whose
    /// title is: one at a time, each with the place its field is.
    @Published var renamingSpace: String?
    @Published var renamingWindow: String?
    /// Why the daemon (or this app) refused the last name typed for a space, by space id.
    @Published var spaceRenameError: [String: String] = [:]
    /// `space_rename` requests in flight, by request id: the space each names.
    var renameReqs: [UInt64: String] = [:]
    /// The `pairing_issue` request still unanswered.
    private var pairingReq: UInt64?

    /// What the request card last drew of its asking session's tool line, for the state query.
    /// Not published: the card writes it after drawing, and nothing reads it to draw.
    private(set) var requestCardShown: (call: String, content: [ToolContent])?

    func recordRequestCard(call: String, content: [ToolContent]) {
        requestCardShown = (call, content)
        Log.info("request card drew \(call) with \(content.count) content items")
    }

    /// The card stopped showing this call's tool line (answered, or the row is gone).
    func clearRequestCard(call: String) {
        guard requestCardShown?.call == call else { return }
        requestCardShown = nil
        Log.info("request card cleared \(call)")
    }

    /// What a session's header says it is doing: a turn blocked on a permission request waits on
    /// the person, so it reads "Waiting on you" instead of the daemon's busy "Working".
    func stateLabel(_ chat: SessionChat) -> String {
        if case let .busy(queued) = chat.state, perms.contains(where: { $0.session == chat.name }) {
            return queued > 0 ? "Waiting on you · \(queued) queued" : "Waiting on you"
        }
        return chat.state?.label ?? "Not running"
    }

    /// Where a waiting request stands among all of them, counting from 1: the count the card
    /// reports.
    func position(of perm: PermInfo) -> Int {
        perms.firstIndex { $0.id == perm.id }.map { $0 + 1 } ?? perms.count
    }
    /// One Ghostty surface per terminal pane, keyed by the term id the tree names.
    private(set) var terminals: [String: PaneTerminal] = [:]
    /// One page per web pane, keyed by pane id.
    private(set) var webPanes: [String: WebPane] = [:]
    /// Browser storage of artifacts whose entries are gone, deleted once no pane shows it.
    private var storeReaper = WebStoreReaper()
    /// One page and its chrome per browser pane, keyed by pane id.
    private(set) var browserPanes: [String: BrowserPane] = [:]
    /// Where each pane on screen sits in the window, in SwiftUI's global space: what `render_pane`
    /// cuts out of the window for a pane that is not a web page. Not published, so a move redraws nothing.
    var paneFrames: [String: CGRect] = [:]
    /// Where each pane's body sits (the pane below its header), same space as `paneFrames`: the
    /// header is the part of the pane above it, which a web page's own snapshot lacks.
    var paneBodyFrames: [String: CGRect] = [:]
    /// Answers the web panes' requests from the daemon.
    private let pageDaemon = PageDaemon()
    /// One Ghostty controller per space, so a space's `terminal.*` settings restyle its terminals
    /// and no other space's. A terminal keeps the controller of the space it was made in.
    private var terminalControllers: [String: TerminalController] = [:]
    /// How each space's terminals draw, from its `terminal.*` settings.
    private(set) var terminalLooks: [String: TerminalAppearance] = [:]
    /// Padding inside the terminal's own background, and the look's font, cursor and colours.
    /// Ghostty's default keybinds are cleared, so a ⌘ key Wheel's keymap leaves unbound reaches the
    /// program in the terminal; the keys the surface still handles itself (copy, paste, select
    /// all, font size) are bound again below.
    private static func terminalConfiguration(_ look: TerminalAppearance) -> TerminalConfiguration {
        TerminalConfiguration(startingFrom: .default) {
            $0.withWindowPaddingX(10)
            $0.withWindowPaddingY(8)
            $0.withCustom("keybind", "clear")
            for bind in [
                "super+c=copy_to_clipboard",
                "super+v=paste_from_clipboard",
                "super+a=select_all",
                "super+equal=increase_font_size:1",
                "super+plus=increase_font_size:1",
                "super+minus=decrease_font_size:1",
                "super+zero=reset_font_size",
            ] {
                $0.withCustom("keybind", bind)
            }
            for line in look.configLines { $0.withCustom(line.key, line.value) }
        }
    }

    /// Ghostty's own light/dark pair for the default theme, else the named theme's two palettes.
    private static func terminalTheme(_ look: TerminalAppearance) -> TerminalTheme {
        func colours(_ palette: TerminalPalette) -> TerminalConfiguration {
            TerminalConfiguration {
                $0.withBackground(palette.background)
                $0.withForeground(palette.foreground)
                for (index, colour) in palette.ansi.enumerated() { $0.withPalette(index, color: colour) }
            }
        }
        guard let light = TerminalPalette.of(look.theme, dark: false), let dark = TerminalPalette.of(look.theme, dark: true) else {
            return .default
        }
        return TerminalTheme(light: colours(light), dark: colours(dark))
    }

    /// The controller for `space`, made on first use from the look held for it (the defaults until
    /// its settings arrive, then restyled live).
    private func terminalController(for space: String?) -> TerminalController {
        let key = space ?? ""
        if let held = terminalControllers[key] { return held }
        let look = terminalLooks[key] ?? .standard
        let made = TerminalController(configuration: Self.terminalConfiguration(look), theme: Self.terminalTheme(look))
        terminalControllers[key] = made
        return made
    }

    /// The space holding the terminal `term`.
    private func spaceId(ofTerm term: String) -> String? {
        spaces.first { space in
            space.root.panes.contains { if case let .terminal(t, _) = $0.kind { return t == term } else { return false } }
        }?.id
    }

    /// The Ghostty config a space's terminals run with, for the debug state.
    func terminalConfig(forTerm term: String) -> String? {
        spaceId(ofTerm: term).flatMap { terminalControllers[$0]?.renderedConfig }
    }
    let endpoint: DaemonEndpoint
    let client: DaemonClient
    /// Session requests (a composer's send, an attach), by session, so a failure shows under
    /// that session's composer.
    private var sessionReqs: [UInt64: String] = [:]
    /// The attaches among `sessionReqs`, and the sessions whose attach the daemon refused (a restore
    /// keeps an agent's session unstarted until its program is found). Every client hears
    /// `session_state_changed` and `session_config`, attached or not, so the first one naming a
    /// refused session is the signal to attach again.
    private var attachReqs: Set<UInt64> = []
    private var refusedAttach: Set<String> = []
    /// Layout ops, so only their failures show above the grid.
    private var layoutReqs: Set<UInt64> = []
    /// Typed address-field text the daemon is reading, by request: the pane and the text, so the
    /// answer reaches that pane and a refusal shows over it.
    private var openReqs: [UInt64: (pane: String, text: String)] = [:]
    /// `<pane>/<term>` of the focus this window last acted on, so a broadcast that leaves it
    /// unchanged does not pull the keyboard back from wherever the person put it.
    private(set) var focusKey: String?
    /// The daemon token, which also guards the debug endpoint.
    let tokenFile: URL
    /// The wheeld this app carries and runs; nil when WHEEL_URL points the app at another daemon
    /// (every check does), or the build has none.
    let service: DaemonService?

    init() {
        let env = ProcessInfo.processInfo.environment
        let override = env["WHEEL_URL"].flatMap(URL.init(string:))
        service = override == nil ? DaemonService.bundled() : nil
        serverControl = service.map(LaunchAgentControl.init) ?? CommandControl(stopCommand: env["WHEEL_SERVER_STOP"], startCommand: env["WHEEL_SERVER_START"])
        let url = override ?? service?.url ?? URL(string: "ws://\(wheelDefaultListen)/ws")!
        tokenFile = env["WHEEL_TOKEN_FILE"].map { URL(filePath: $0) }
            ?? service?.tokenFile
            ?? FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Application Support/wheel/token")
        let endpoint = DaemonEndpoint(url: url, token: DaemonEndpoint.tokenFile(tokenFile))
        self.endpoint = endpoint
        client = DaemonClient(endpoint: endpoint)
        client.log = { Log.info($0) }
        client.appLog = Log.file
        client.onConnection = { [weak self] state in
            self?.connection = state
            if state != .connected { self?.usage.disconnected() }
            if state != .connected { self?.links.forget() }
            if state != .connected { self?.terminals.values.forEach { $0.attached = false } }
            // A shipping state from a daemon this app no longer reaches says nothing about now.
            if state != .connected { self?.settings.logShipping = nil }
        }
        client.onConnect = { [weak self] status in self?.connected(status) }
        client.onMessage = { [weak self] msg in self?.handle(msg) }
        rebuildKeymap()
    }

    func start() {
        Log.info("app start, daemon \(endpoint.url.absoluteString)")
        installKeyMonitor()
        #if DEBUG
        if let marks = ProcessInfo.processInfo.environment["WHEEL_LOG_MARKS"] { Log.file.writeMarks(marks) }
        #endif
        // A server stopped from the menu stays stopped until it is started from there.
        if serverStoppedByMenu { Log.info("server was stopped from the menu; not starting it") } else { service?.register() }
        client.start()
        // The daemon reads at most once per its interval however often this asks.
        usageTimer = Timer.scheduledTimer(withTimeInterval: 300, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.askUsage(dueOnly: true) }
        }
    }

    /// The space the focused window shows; nil when there are none.
    var active: String? { layout.focusedWindow?.active }
    var activeSpace: SpaceInfo? { spaces.first { $0.id == active } }
    /// Whether the focused window's sidebar lists the spaces.
    var sidebarOpen: Bool { layout.focusedWindow?.sidebar ?? true }

    /// Send the daemon a frame that is not a request of this model's own.
    func reply(_ msg: ClientMsg) { client.send(msg) }

    /// The daemon's status at connect carries the whole layout; every terminal pane and every
    /// session reattaches.
    private func connected(_ status: ServerMsg) {
        // Answers to the last connection's requests never come.
        sessionReqs = [:]
        attachReqs = []
        refusedAttach = []
        for open in openReqs.values {
            browserPanes[open.pane]?.refused(typed: open.text, "The connection to wheeld dropped before it read this address.")
        }
        openReqs = [:]
        client.send(.renderOffer)
        // This app owns the windows: `wheel window` commands come here through the daemon.
        client.send(.windowOffer)
        settings.forgetRequests()
        // Answers to the last connection's palette listings never come.
        paletteReqs = [:]
        recentReq = nil
        dirReqs = [:]
        rootsReq = nil
        runnersReq = nil
        if commandPalette != nil { askRecentDirs(); askRunners() }
        if commandPalette?.mode == .newSpace { askRoots(); askDirList(commandPalette?.browsedParent) }
        let known = Array(chats.values)
        if case let .status(_, _, _, uptime, _, perms, spaces, active, agents, shellIntegration, loginPath, logs, host) = status {
            self.agents = agents
            settings.commandLine.shellProblem = shellIntegration
            settings.commandLine.loginPath = loginPath
            settings.hostAccess = host
            showLogShipping(logs)
            serverStarted = Date().addingTimeInterval(-Double(uptime))
            // The restarted daemon closes this connection; the client reconnects to the new one.
            if service?.restartIfOlder(uptimeSecs: uptime, exe: host?.exe) == true { return }
            self.perms = perms
            apply(spaces, active)
        }
        // A palette open across a reconnect lists again, against the layout the new daemon sent.
        for pane in palettes.keys { askArtifacts(pane) }
        if settings.open { askSettings() }
        askUsage(dueOnly: true)
        askRemote()
        askStatusDots()
        appearances.keep(only: spaces, reset: true)
        appearanceReqs = [:]
        askAppearances()
        sendPairingCancels()
        // `apply` attached the sessions it added; the rest were attached on an earlier connection.
        for chat in known where chats[chat.name] === chat { attach(chat) }
        for terminal in terminals.values { attach(terminal) }
        Task {
            do {
                pageDaemon.token = try await endpoint.token()
                pageDaemon.url = endpoint.url
            } catch {
                Log.info("no token for web panes: \(error.localizedDescription)")
                return
            }
            // A page loaded before this connect may be stale (the daemon restarted, files changed).
            for web in webPanes.values { web.loaded ? web.reload() : open(web) }
        }
    }

    /// Mirror a new layout: make a surface for each terminal pane that appeared, drop the ones
    /// whose pane went, and move keyboard focus when the daemon moved it. Returns the new
    /// surfaces, which the caller attaches.
    @discardableResult
    private func apply(_ spaces: [SpaceInfo], _ active: String?) -> [PaneTerminal] {
        let old = self.spaces
        // A split, close, swap or move changes a tree; a status or focus report does not, so it never animates.
        // The first report after a connect has nothing to move from, so it applies without animation.
        let animated = !old.isEmpty && !(old.count == spaces.count && zip(old, spaces).allSatisfy { $0.root.sameShape(as: $1.root) })
        if animated {
            withAnimation(GridMotion.layout) { self.spaces = spaces }
        } else {
            self.spaces = spaces
        }
        daemonActive = active
        reconcileWindows(spaces, daemonActive: active)
        // A background space has no window to keep an empty pane in, so it goes with its last pane (ADR-0014).
        // wheeld refuses the close when something started in the space since this report.
        for space in layout.emptied(from: old, to: spaces) {
            Log.info("background space \(space) lost its last pane; closing it")
            sendLayoutOp { .spaceClose(req: $0, id: space, ifEmpty: true) }
        }
        refreshCommandPalette()
        let held = Set(spaces.flatMap { $0.root.panes.map { $0.id } })
        rawContextPanes.formIntersection(held)
        paneFrames = paneFrames.filter { held.contains($0.key) }
        paneBodyFrames = paneBodyFrames.filter { held.contains($0.key) }
        // A palette is for the pane as it was when it opened: a pane that left, or that something
        // else (an agent, the CLI) changed to another kind, closes it.
        let kinds = Dictionary(uniqueKeysWithValues: spaces.flatMap { $0.root.panes }.map { ($0.id, $0.kind.name) })
        for (pane, palette) in palettes where kinds[pane] != palette.openedOver {
            palettes[pane] = nil
            Log.info("palette \(pane): its pane \(kinds[pane] == nil ? "left the layout" : "became \(kinds[pane] ?? "")"); closed")
        }
        keepSettingsProject()
        settle()
        // A terminal keeps the controller it was made with, so one moved to another space is made
        // again with that space's: detached and re-attached, its shell untouched.
        var rebuiltFocused: Set<String> = []
        for (term, terminal) in terminals {
            guard let now = spaceId(ofTerm: term), now != terminal.space else { continue }
            if terminal.view.isFocused { rebuiltFocused.insert(term) }
            Log.info("terminal \(term) moved from space \(terminal.space ?? "none") to \(now); surface rebuilt with its look")
            client.send(.termDetach(id: term))
            terminals[term] = nil
        }
        let plan = SurfacePlan(spaces, active: self.active, shown: shownSpaces, existing: Set(terminals.keys), focusKey: focusKey)
        for term in plan.removed {
            retire("term:\(term)", animated: animated, stillWanted: { [self] in spaceId(ofTerm: term) != nil }) { [self] in
                client.send(.termDetach(id: term))
                terminals[term] = nil
                Log.info("terminal \(term) left the layout; detached")
            }
        }
        let added = plan.added.map { term in
            let space = spaceId(ofTerm: term)
            let terminal = PaneTerminal(term: term, space: space, controller: terminalController(for: space), model: self)
            terminals[term] = terminal
            return terminal
        }
        for (term, visible) in plan.visible { terminals[term]?.view.isSurfaceVisible = visible }
        let wanted = WebPanes.panes(spaces)
        for (pane, web) in webPanes where wanted[pane]?.artifact != web.artifact {
            let leaving = !spaces.contains { $0.root.panes.contains { $0.id == pane } }
            retire("web:\(pane)", animated: animated && leaving, stillWanted: { [self] in WebPanes.panes(self.spaces)[pane]?.artifact == web.artifact }) { [self] in
                webPanes[pane] = nil
                Log.info("web pane \(pane) left the layout")
                reapGoneStores(shown: Set(WebPanes.panes(spaces).values.map(\.store)))
            }
        }
        for (pane, shown) in wanted where webPanes[pane] == nil {
            storeReaper.wanted(shown.store)
            let web = WebPane(pane: pane, artifact: shown.artifact, store: shown.store, daemon: pageDaemon)
            webPanes[pane] = web
            open(web)
        }
        reapGoneStores(shown: Set(wanted.values.map(\.store)))
        let pages = BrowserPanes.urls(spaces)
        for pane in browserPanes.keys where pages[pane] == nil {
            retire("browser:\(pane)", animated: animated, stillWanted: { [self] in BrowserPanes.urls(spaces)[pane] != nil }) { [self] in
                browserPanes[pane] = nil
                Log.info("browser pane \(pane) left the layout")
            }
        }
        for (pane, url) in pages {
            if let browser = browserPanes[pane] {
                browser.daemonSays(url)
            } else {
                Log.info("browser pane \(pane) opening \(url)")
                browserPanes[pane] = BrowserPane(
                    pane: pane, url: url, daemonIsRemote: !endpoint.isLoopback,
                    report: { [weak self] in self?.reportPage(pane, $0) },
                    ask: { [weak self] in self?.openTyped(pane, $0) },
                    console: { [weak self] reset, entries in
                        self?.client.send(.paneConsole(req: nil, pane: pane, reset: reset, entries: entries))
                    }
                )
            }
        }
        let sessions = Set(AgentPanes.sessions(spaces).values)
        for name in chats.keys where !sessions.contains(name) {
            retire("session:\(name)", animated: animated, stillWanted: { [self] in AgentPanes.sessions(spaces).values.contains(name) }) { [self] in
                client.send(.sessionDetach(session: name))
                chats[name] = nil
                Log.info("session \(name) left the layout; detached")
            }
        }
        for name in sessions where chats[name] == nil {
            let chat = SessionChat(name: name)
            chats[name] = chat
            attach(chat)
        }
        appearances.keep(only: spaces)
        let live = Set(spaces.map(\.id))
        terminalLooks = terminalLooks.filter { live.contains($0.key) }
        terminalControllers = terminalControllers.filter { live.contains($0.key) }
        askAppearances()
        let layoutLog = "spaces: \(spaces.map { "\($0.id)(\($0.root.panes.map { "\($0.id):\($0.kind.name)" }.joined(separator: ",")))" }.joined(separator: " ")) active=\(active ?? "none")"
        if layoutLog != lastLayoutLog {
            lastLayoutLog = layoutLog
            Log.info(layoutLog)
        }
        focusKey = plan.focusKey
        if let term = plan.focus { terminals[term]?.view.requestFocus() }
        else if let term = rebuiltFocused.first { terminals[term]?.view.requestFocus() }
        return added
    }

    /// Surfaces whose pane left a grid that is animating, until the pane has shrunk away.
    private var lingering: Set<String> = []

    /// Drop a surface whose pane left the layout. While the grid animates, the closing pane is
    /// still drawn shrinking, so its surface stays until the animation ends (`drop` is skipped if
    /// the pane came back by then).
    private func retire(_ key: String, animated: Bool, stillWanted: @escaping () -> Bool, _ drop: @escaping () -> Void) {
        // A report that comes while a surface lingers (a status update) leaves it to its timer.
        guard !lingering.contains(key) else { return }
        guard animated else {
            drop()
            return
        }
        lingering.insert(key)
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(GridMotion.duration + 0.1))
            guard let self, lingering.remove(key) != nil, !stillWanted() else { return }
            drop()
        }
    }

    /// Delete the stores of removed artifacts that no pane shows. A store a live page still uses
    /// refuses to go (WebKit's network process lets go of a closed pane's store a moment late); it
    /// stays pending and is tried again a second later, up to five times in all. After the fifth
    /// failure it is left alone (and logged once) until a pane shows it again. A store no page ever made is only forgotten.
    private func reapGoneStores(shown: Set<UUID>) {
        let held = shown.union(webPanes.values.map(\.store))
        let due = storeReaper.start(shown: held)
        // WebKit's data store calls crash in a process where nothing has touched WebKit yet (no web
        // pane opened since launch); the default store sets it up.
        if !due.isEmpty { _ = WKWebsiteDataStore.default() }
        for store in due {
            Task { @MainActor in
                guard await WKWebsiteDataStore.allDataStoreIdentifiers.contains(store) else {
                    Log.info("page store \(store.uuidString) was never made; nothing to remove")
                    storeReaper.finished(store, deleted: true)
                    return
                }
                do {
                    try await WKWebsiteDataStore.remove(forIdentifier: store)
                    Log.info("removed page store \(store.uuidString)")
                    storeReaper.finished(store, deleted: true)
                } catch {
                    if storeReaper.finished(store, deleted: false) {
                        Log.info("could not remove page store \(store.uuidString): \(error.localizedDescription); giving up after \(WebStoreReaper.maxFailures) tries, leaving it until a pane shows it again")
                        return
                    }
                    Log.info("could not remove page store \(store.uuidString): \(error.localizedDescription); will try again")
                    try? await Task.sleep(for: .seconds(1))
                    reapGoneStores(shown: Set(WebPanes.panes(spaces).values.map(\.store)))
                }
            }
        }
    }

    private func handle(_ msg: ServerMsg) {
        if links.received(msg) { return }
        if receivedBuiltins(msg) { return }
        switch msg {
        case let .status(req, _, _, _, _, _, _, _, _, _, _, logs, host) where req != nil && req == settings.statusReq:
            // Settings asking after the log shipping and folder access: the rest of the reply is no news.
            settings.statusReq = nil
            showLogShipping(logs)
            settings.hostAccess = host
        case let .status(_, _, _, _, _, perms, spaces, active, agents, shellIntegration, loginPath, logs, host):
            self.agents = agents
            showLogShipping(logs)
            settings.hostAccess = host
            settings.commandLine.shellProblem = shellIntegration
            settings.commandLine.loginPath = loginPath
            // An unsolicited status means this connection missed daemon-wide events.
            self.perms = perms
            apply(spaces, active).forEach(attach)
        case let .spaces(spaces, active):
            // The layout broadcast leaves statuses out; `status_changed` carries them.
            let held = Dictionary(uniqueKeysWithValues: self.spaces.map { ($0.id, $0) })
            apply(spaces.map { $0.keepingStatuses(of: held[$0.id]) }, active).forEach(attach)
        case let .statusChanged(space, pane, status, spaceStatus):
            spaces = spaces.map { $0.id == space ? $0.applying(pane: pane, status: status, spaceStatus: spaceStatus) : $0 }
            Log.info("status \(space)/\(pane ?? "-"): pane=\(status.map(\.label) ?? "none") space=\(spaceStatus.map(\.label) ?? "none")")
        case let .renderPane(id, pane):
            renderPane(id: id, pane: pane)
        case let .windowRequest(id, command):
            answerWindowCommand(id: id, command)
        case let .paneReload(pane):
            webPanes[pane]?.reload()
        case let .spaceFocused(space):
            spaceFocusRequested(space)
        case let .artifactGone(scope, name, cwd):
            let store = storeReaper.gone(scope: scope, name: name, cwd: cwd)
            Log.info("artifact \(scope.rawValue) \(name) is gone; its page store \(store.uuidString) goes once no pane shows it")
            reapGoneStores(shown: Set(WebPanes.panes(spaces).values.map(\.store)))
        case let .paneUrl(pane, url):
            // One field of one pane, so the layout is patched in place rather than replaced: the
            // daemon sends this instead of the whole layout because a person browsing changes it
            // on every page load. A pane this client does not hold is not an error — the next
            // `spaces` or `status` carries the truth.
            spaces = spaces.map { space in
                var space = space
                space.root = space.root.replacing(pane: pane, with: .browser(url: url))
                return space
            }
            browserPanes[pane]?.daemonSays(url)
        case let .settings(req, space, list):
            received(settings: list, for: space, req: req)
        case let .artifacts(req, list):
            if let req, req == paletteArtifactsReq {
                paletteArtifactsReq = nil
                paletteArtifacts = list
                Log.info("command palette: \(list.count) artifacts listed")
                refreshCommandPalette()
            }
            if let req, let pane = paletteReqs.removeValue(forKey: req) {
                palettes[pane]?.state.listed(list)
                Log.info("palette \(pane): \(list.count) artifacts listed")
            }
        case let .spaceRecent(req, dirs):
            guard req == recentReq else { return }
            recentReq = nil
            recentDirs = dirs
            Log.info("command palette: \(dirs.count) recent directories listed")
            refreshCommandPalette()
        case let .agentHomes(req, homes, checked, error):
            guard let ask = settings.homeReqs.removeValue(forKey: req), let agent = SettingsModel.homeAgent(ask.field.key) else { return }
            Log.info("agent homes \(agent): \(homes.count) found, checked \(checked.map { "\($0.path) \($0.state)" } ?? "-")\(error.map { ", error \($0)" } ?? "")")
            settings.homesListed(ask, agent: agent, homes: homes, checked: checked)
        case let .dirRoots(req, roots):
            guard req == rootsReq else { return }
            rootsReq = nil
            Log.info("command palette: roots \(roots)")
            commandPalette?.setRoots(roots)
            for root in roots { askDirList(root) }
        case let .dirList(req, path, dirs, error):
            guard let parent = dirReqs.removeValue(forKey: req) else { return }
            Log.info("command palette: \(parent) -> \(path): \(dirs.count) folders\(error.map { ", error \($0)" } ?? "")")
            commandPalette?.listed(parent, .init(dirs: dirs, error: error))
        case let .runners(req, list):
            guard req == runnersReq else { return }
            runnersReq = nil
            runners = list
            Log.info("command palette: \(list.count) runners listed: \(list.map(\.name).joined(separator: ","))")
            refreshCommandPalette()
        case let .created(req, id):
            if layoutReqs.remove(req) != nil { Log.info("created \(id)") }
            if let info = splitArtifacts.removeValue(forKey: req) {
                Log.info("command palette: \(info.id) opens in \(id)")
                setKind(id, .web, artifact: info.name, scope: info.scope)
            }
        case let .started(req, pane, session):
            if layoutReqs.remove(req) != nil { Log.info("started \(pane) session \(session ?? "-")") }
        case let .paneView(req, pane, view, conversation):
            if layoutReqs.remove(req) != nil { Log.info("pane \(pane) view \(view?.rawValue ?? "-") conversation \(conversation ?? "-")") }
        case let .termAttached(_, id, replayBytes):
            guard let terminal = terminals[id] else { return }
            // Every attach (and every resync) replays from scratch: reset the screen first.
            terminal.session.receive(Data("\u{1b}c".utf8))
            terminal.attached = true
            Log.info("attached to \(id), replay \(replayBytes) bytes")
        case let .termOutput(id, data):
            terminals[id]?.session.receive(data)
        case let .termExited(id, code):
            guard let terminal = terminals[id] else { return }
            Log.info("terminal \(id) exited with \(code.map(String.init) ?? "?")")
            terminal.session.receive(Data("\r\n[shell exited — close this pane and split for a new one]\r\n".utf8))
        case let .remote(_, info):
            settings.devices.received(info)
            Log.info("remote listen=\(info.listen ?? "off") port=\(info.port) addresses=\(info.addresses.joined(separator: ",")) fingerprint=\(info.fingerprint) problem=\(info.problem ?? "-")")
        case let .pairing(req, token, expiresMs):
            guard req == pairingReq else { break }
            pairingReq = nil
            settings.devices.received(token: token, expiresMs: expiresMs)
            Log.info("pairing code issued, expires \(expiresMs)")
            pairingCodeArrived()
        case let .deviceConnected(_, name, address, paired):
            deviceConnected(name: name, address: address, paired: paired)
        case let .devices(_, devices), let .devicesChanged(devices):
            settings.devices.received(devices: devices)
            Log.info("devices: \(devices.map { "\($0.id) \($0.name) connected=\($0.connected)" }.joined(separator: ", "))")
        case let .usage(_, board), let .usageChanged(board):
            usage.received(board)
            Log.info("usage board agents=\(board.agents.map { "\($0.id):shown=\($0.shown):readable=\($0.readable):accounts=\($0.accounts.map { "\($0.name)/\($0.usage.kind.rawValue)" }.joined(separator: "+"))" }.joined(separator: ",")) pins=\(board.pins.joined(separator: ","))")
        case let .sessionStateChanged(session, state):
            chats[session]?.state = state
            attachAgainIfRefused(session)
        case let .sessionAttached(req, session, state, events, config, context, now):
            if let req { sessionReqs[req] = nil; attachReqs.remove(req) }
            refusedAttach.remove(session)
            guard let chat = chats[session] else { return }
            chat.state = state
            chat.reset(events, daemonNow: now)
            chat.setConfig(config)
            chat.context = context
            chat.error = nil
            chat.notice = nil
            chat.title = nil
            Log.info("session \(session) attached, \(events.count) events, \(config.count) config options, context \(context?.helpText ?? "unknown")")
        case let .conversation(req, session, id):
            sessionReqs[req] = nil
            Log.info("session \(session) is in conversation \(id)")
        case let .sessionConfig(session, options):
            attachAgainIfRefused(session)
            guard let chat = chats[session] else { return }
            chat.setConfig(options)
            Log.info("session \(session) config: \(options.map { "\($0.id)=\($0.currentLabel ?? "?")" }.joined(separator: " "))")
        case let .sessionContext(session, context):
            guard let chat = chats[session] else { return }
            chat.context = context
            Log.info("session \(session) context: \(context?.helpText ?? "unknown")")
        case let .sessionCommands(session, commands):
            chats[session]?.commands = Set(commands)
            Log.info("session \(session) accepts \(commands.count) slash commands")
        case let .sessionTitle(session, title):
            chats[session]?.title = title
            Log.info("session \(session) title: \(title ?? "none")")
        case let .paneInfo(_, pane, kind, fields):
            paneInfo[pane] = fields
            Log.info("pane \(pane) info (\(kind)): \(fields.map { "\($0.label)=\($0.value)" }.joined(separator: " "))")
        case let .sessionNotice(session, notice):
            guard let chat = chats[session] else { return }
            chat.notice = notice
            Log.info("session \(session) notice: \(notice ?? "down")")
        case let .session(session, event):
            chats[session]?.apply(event)
        case let .permRequested(perm):
            if !perms.contains(where: { $0.id == perm.id }) { perms.append(perm) }
            Log.info("permission \(perm.id) waiting: \(perm.title)")
        case let .permResolved(id, choice):
            perms.removeAll { $0.id == id }
            Log.info("permission \(id) resolved: \(choice)")
        case let .sessionQueued(req, _, _):
            sessionReqs[req] = nil
        case let .opened(req, pane, url):
            if openReqs.removeValue(forKey: req) != nil { browserPanes[pane]?.opened(url) }
        case let .ok(req):
            layoutReqs.remove(req)
            if let space = renameReqs.removeValue(forKey: req) {
                spaceRenameError[space] = nil
                if renamingSpace == space { renamingSpace = nil }
                Log.info("space \(space) renamed")
            }
        case let .error(req, term, message):
            if let req, let space = renameReqs.removeValue(forKey: req) {
                spaceRenameError[space] = message
                Log.info("space \(space) rename refused: \(message)")
            }
            if let req, req == pairingReq {
                pairingReq = nil
                settings.devices.codeProblem = message
            }
            if let req, req == runnersReq {
                runnersReq = nil
                runners = []
                refreshCommandPalette()
                Log.info("command palette: runner ls refused: \(message)")
            }
            if let req, let session = sessionReqs.removeValue(forKey: req) {
                chats[session]?.error = message
                if attachReqs.remove(req) != nil { refusedAttach.insert(session) }
            }
            if let req, let open = openReqs.removeValue(forKey: req) { browserPanes[open.pane]?.refused(typed: open.text, message) }
            if let req { splitArtifacts[req] = nil }
            if let req, layoutReqs.remove(req) != nil {
                layoutError = message
                settle()
            }
            if let req, let field = settings.changeReqs.removeValue(forKey: req) {
                Log.info("setting \(field.key) (\(field.layer.rawValue)) refused: \(message)")
                settings.refused(field, message)
            }
            if let req, let pane = paletteReqs.removeValue(forKey: req) {
                palettes[pane]?.state.failed(message)
                Log.info("palette \(pane): listing refused: \(message)")
            }
            if let req, req == settings.listReq {
                settings.listReq = nil
                settings.listFailed(message)
            }
            Log.info("daemon error\(term.map { " on \($0)" } ?? ""): \(message)")
        default:
            break
        }
    }

    /// Whether `pane` is the focused pane of the space on screen: the one a ⌘↩ answers for.
    func paneIsFocused(_ pane: String) -> Bool {
        spaces.first { $0.id == active }?.focus == pane
    }

    // MARK: Layout ops — the tab bar, pane headers, dividers and the debug endpoint all call these.

    func sendLayoutOp(clearsError: Bool = true, _ make: (UInt64) -> ClientMsg) {
        if clearsError { layoutError = nil }
        let req = client.req()
        layoutReqs.insert(req)
        let msg = make(req)
        Log.info("layout op: \((try? msg.json()) ?? "?")")
        client.send(msg)
    }

    func createSpace(cwd: String) { sendLayoutOp { .spaceCreate(req: $0, cwd: cwd) } }
    func closeSpace(_ id: String) { sendLayoutOp { .spaceClose(req: $0, id: id) } }
    /// Tell the daemon `space` is the one in use: what a CLI command with no space acts on.
    func daemonFocus(space: String) {
        sendLayoutOp(clearsError: false) { .focus(req: $0, space: space, pane: nil) }
    }
    func focus(pane: String) { sendLayoutOp(clearsError: false) { .focus(req: $0, space: nil, pane: pane) } }
    /// Split `pane` on `dir`; with none, the daemon places the new pane by `panes.placement`.
    func split(_ pane: String, _ dir: Dir?) {
        // The new pane would sit hidden behind the zoomed one.
        if let space = space(holding: pane) { zoomedPanes[space.id] = nil }
        sendLayoutOp { .paneSplit(req: $0, pane: pane, dir: dir) }
    }

    /// Show artifact `info` in a new pane split right of the focused one: split now, and the
    /// daemon's `created` answer for the split names the pane to switch to the artifact.
    func openArtifactInNewPane(_ info: ArtifactInfo) {
        guard connection == .connected, let focused = activeSpace?.focus else { return }
        if let space = space(holding: focused) { zoomedPanes[space.id] = nil }
        let req = client.req()
        splitArtifacts[req] = info
        Log.info("command palette: split \(focused) right for \(info.id)")
        sendLayoutOp(.paneSplit(req: req, pane: focused, dir: .right), req: req)
    }

    /// The pane `space` shows alone: one still in that space and holding its focus, so keys never go to a hidden pane.
    func zoomedPane(in space: SpaceInfo) -> String? {
        guard let pane = zoomedPanes[space.id], space.focus == pane, space.root.panes.contains(where: { $0.id == pane }) else { return nil }
        return pane
    }

    /// Show `pane` alone in its space, or put the grid back when it already is.
    func toggleZoom(_ pane: String) {
        guard let space = space(holding: pane) else { return }
        let zoomed = zoomedPane(in: space) != pane
        if zoomed { focus(pane: pane) }
        withAnimation(GridMotion.layout) { zoomedPanes[space.id] = zoomed ? pane : nil }
        Log.info("pane \(pane): \(zoomed ? "zoomed" : "unzoomed")")
    }

    /// One entry of a pane header's context menu.
    struct HeaderMenuItem: Equatable {
        let id: String
        let title: String
        let enabled: Bool
    }

    /// What right-clicking `pane`'s header offers; the header draws it and the debug state reports it.
    func headerMenu(for pane: String, kind: PaneKind) -> [HeaderMenuItem] {
        var items: [HeaderMenuItem] = []
        if case .unknown = kind {} else {
            for dir in [Dir.right, .left, .down, .up] {
                items.append(HeaderMenuItem(id: "split-\(dir.rawValue)", title: "Split \(dir.rawValue.capitalized)", enabled: true))
            }
        }
        let space = space(holding: pane)
        let zoomed = space.flatMap { zoomedPane(in: $0) } == pane
        items.append(HeaderMenuItem(id: "zoom", title: zoomed ? "Unzoom Pane" : "Zoom Pane",
                                    enabled: zoomed || (space?.root.panes.count ?? 0) > 1))
        for group in paneMoveGroups(for: pane) {
            for row in group.rows {
                items.append(HeaderMenuItem(id: "move-space:\(row.space)", title: "Move Pane to Space ▸ \(group.title) ▸ \(row.title)\(row.here ? " (here)" : "")", enabled: !row.here))
            }
        }
        if case .unknown = kind {} else {
            let isShell: Bool = { if case let .terminal(_, agent) = kind { return agent == nil }; return false }()
            let isBrowser: Bool = { if case .browser = kind { return true }; return false }()
            let isEmpty: Bool = { if case .empty = kind { return true }; return false }()
            items.append(HeaderMenuItem(id: "kind:shell", title: "Change Type ▸ Shell", enabled: !isShell))
            for agent in AgentInfo.pickable(agents) {
                let current: Bool = { if case let .agent(_, name) = kind { return name == agent.name }; return false }()
                items.append(HeaderMenuItem(id: "kind:agent:\(agent.name)", title: "Change Type ▸ \(agent.title)", enabled: !current))
            }
            items.append(HeaderMenuItem(id: "kind:browser", title: "Change Type ▸ Browser", enabled: !isBrowser))
            items.append(HeaderMenuItem(id: "kind:artifact", title: "Change Type ▸ Artifact…", enabled: true))
            items.append(HeaderMenuItem(id: "kind:empty", title: "Change Type ▸ Empty", enabled: !isEmpty))
        }
        items.append(HeaderMenuItem(id: "move-new-space", title: "Move Pane to New Space", enabled: true))
        items.append(HeaderMenuItem(id: "move-new-little", title: "Move Pane to New Little Window", enabled: true))
        items.append(HeaderMenuItem(id: "close", title: "Close Pane", enabled: true))
        return items
    }

    /// Run a header menu item other than Close, which the header confirms first.
    func runHeaderMenu(_ id: String, pane: String) {
        if id == "zoom" { toggleZoom(pane); return }
        switch id {
        case "kind:shell": askSetKind(pane, .terminal); return
        case "kind:browser": askSetKind(pane, .browser); return
        case "kind:empty": askSetKind(pane, .empty); return
        case "kind:artifact": openPalette(pane); return
        default: break
        }
        if id.hasPrefix("kind:agent:") { askStartAgent(pane, String(id.dropFirst("kind:agent:".count))); return }
        if id.hasPrefix("move-space:") { movePane(pane, toEndOf: String(id.dropFirst("move-space:".count))); return }
        if id == "move-new-space" { movePaneToNewSpace(pane); return }
        if id == "move-new-little" { movePaneToNewSpace(pane, little: true); return }
        if id.hasPrefix("split-"), let dir = Dir(rawValue: String(id.dropFirst("split-".count))) { split(pane, dir) }
    }
    /// A click on a pane's context meter: flip that pane between the percent and the raw figures.
    func toggleContextLabel(_ pane: String) {
        if rawContextPanes.remove(pane) == nil { rawContextPanes.insert(pane) }
        Log.info("pane \(pane) context label: \(rawContextPanes.contains(pane) ? "raw" : "percent")")
    }

    func closePane(_ pane: String) {
        if let space = space(holding: pane) { zoomedPanes[space.id] = nil }
        sendLayoutOp { .paneClose(req: $0, pane: pane) }
    }
    func setKind(_ pane: String, _ kind: NewPaneKind, artifact: String? = nil, scope: ArtifactScope? = nil) {
        sendLayoutOp { .paneSetKind(req: $0, pane: pane, kind: kind, artifact: artifact, scope: scope) }
    }
    /// The fields of each pane's ⓘ popover as the daemon last listed them, by pane id.
    @Published private(set) var paneInfo: [String: [InfoField]] = [:]
    /// Ask the daemon what `pane` is; the answer lands in `paneInfo`. The last answer is dropped
    /// first, so the popover shows its spinner rather than values from an earlier open.
    func requestPaneInfo(_ pane: String) {
        guard connection == .connected else { return }
        paneInfo[pane] = nil
        client.send(.paneInfo(req: client.req(), pane: pane))
    }
    /// Show the pane's Claude Code conversation as `view`; a refusal shows above the grid.
    func switchView(_ pane: String, to view: ConversationView) {
        sendLayoutOp { .paneView(req: $0, pane: pane, to: view) }
    }
    func startPane(_ pane: String, _ kind: StartKind, message: String? = nil, agent: String? = nil) {
        sendLayoutOp { .paneStart(req: $0, pane: pane, kind: kind, message: message, agent: agent) }
    }
    /// Whether the daemon found the program of agent `name`.
    func agentFound(_ name: String) -> Bool { agents.contains { $0.name == name && $0.found } }
    func setRatio(_ split: String, _ ratio: Double) {
        // Offline the op goes nowhere and no answer comes, so the divider snaps back now.
        guard connection == .connected else {
            dragRatios[split] = nil
            return
        }
        settling.insert(split)
        sendLayoutOp { .splitRatio(req: $0, split: split, ratio: ratio) }
    }
    /// Draw a divider at `ratio` while it is dragged; `setRatio` at the drag's end tells the daemon.
    func dragRatio(_ split: String, _ ratio: Double) { dragRatios[split] = ratio }

    // MARK: Pane drag — a header drag swaps a pane or docks it beside another.

    /// Start dragging `pane`; Escape cancels until the pointer is released.
    /// The gesture calls this on every move; after Escape it does nothing until the pointer is
    /// released. `fresh` (the debug endpoint, which has no gesture) forgets an earlier Escape.
    /// `grabbedAt` is where the pointer pressed, in the grid's coordinates: the chip keeps that offset from its corner.
    func beginPaneDrag(_ pane: String, fresh: Bool = false, grabbedAt: CGPoint? = nil) {
        if fresh { dragCancelled = false }
        guard paneDrag == nil, !dragCancelled, connection == .connected, let home = space(holding: pane) else { return }
        paneDrag = PaneDrag(pane: pane, byPointer: !fresh)
        if let grabbedAt {
            // A zoomed pane's header sits at the grid's own corner.
            let origin = zoomedPane(in: home) == pane ? .zero : baseLayouts[home.id]?.panes.first(where: { $0.id == pane })?.frame.origin
            paneGrab = origin.map { CGSize(width: grabbedAt.x - $0.x, height: grabbedAt.y - $0.y) }
        } else {
            paneGrab = nil
        }
        showPaneChip(at: NSEvent.mouseLocation)
        // The header's gesture can go away mid-drag (the space it is in stops showing while a tab's
        // hold opens another), so the pointer's moves and its release are watched here as well.
        dragMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .leftMouseDragged, .leftMouseUp]) { [weak self] event in
            guard let self, let drag = self.paneDrag else { return event }
            switch event.type {
            case .leftMouseDragged:
                if drag.byPointer { MainActor.assumeIsolated { self.movePaneDrag() } }
                return event
            case .leftMouseUp:
                if drag.byPointer { MainActor.assumeIsolated { self.endPaneDrag() } }
                return event
            default:
                guard event.keyCode == 53 else { return event }
                MainActor.assumeIsolated { self.cancelPaneDrag(escape: true) }
                return nil
            }
        }
        Log.info("pane drag \(pane): started")
    }

    /// The pointer is over `drop`, or over no other pane. The layout animates to show the result.
    func hoverPaneDrag(_ drop: PaneDrop?) {
        guard var drag = paneDrag, !drag.committed, drag.drop != drop else { return }
        if drop?.target == drag.pane { return hoverPaneDrag(nil) }
        // A pane in another space is a dock there, which the pointer's hit test also makes.
        if let drop, let home = space(holding: drag.pane)?.id, let there = space(holding: drop.target), there.id != home {
            let dir: Dir = if case let .edge(dir) = drop.zone { dir } else { .right }
            return setPaneAway(.beside(pane: drop.target, dir: dir, space: there.id))
        }
        drag.drop = drop
        if drop != nil { drag.away = nil }
        withAnimation(GridMotion.layout) { paneDrag = drag }
        Log.info("pane drag \(drag.pane): over \(drop.map { "\($0.target) \($0.zone.label)" } ?? "nothing")")
    }

    /// Pointer position in the grid's space: hover whatever pane is there.
    func dragPane(to point: CGPoint) {
        guard let drag = paneDrag, !dragCancelled, let id = space(holding: drag.pane)?.id, let base = baseLayouts[id] else { return }
        // Over a tab, a row, another space or the desktop the drop is theirs, not the grid's.
        if drag.away != nil { return }
        hoverPaneDrag(PaneDragging.drop(at: point, among: base.panes, dragging: drag.pane))
    }

    /// Release: send the drop the pointer is over, or cancel when it is over none.
    func endPaneDrag() {
        let cancelled = dragCancelled
        dragCancelled = false
        guard var drag = paneDrag, !drag.committed else { return }
        removeDragMonitor()
        guard !cancelled, drag.drop != nil || drag.away != nil, connection == .connected else {
            withAnimation(GridMotion.layout) { paneDrag = nil }
            endAway()
            Log.info("pane drag \(drag.pane): cancelled")
            return
        }
        drag.committed = true
        paneDrag = drag
        dragChip?.hide()
        if let away = drag.away {
            dropAway(drag.pane, away)
            return
        }
        guard let drop = drag.drop else { return }
        Log.info("pane drag \(drag.pane): dropped on \(drop.target) \(drop.zone.label)")
        switch drop.zone {
        case .center: sendLayoutOp { .paneSwap(req: $0, pane: drag.pane, with: drop.target) }
        case let .edge(dir): sendLayoutOp { .paneMove(req: $0, pane: drag.pane, to: .beside(pane: drop.target, dir: dir)) }
        }
    }

    func cancelPaneDrag(escape: Bool = false) {
        guard let drag = paneDrag, !drag.committed else { return }
        if escape { dragCancelled = true }
        removeDragMonitor()
        withAnimation(GridMotion.layout) { paneDrag = nil }
        endAway()
        Log.info("pane drag \(drag.pane): cancelled\(escape ? " by escape" : "")")
    }

    func removeDragMonitor() {
        if let dragMonitor { NSEvent.removeMonitor(dragMonitor) }
        dragMonitor = nil
    }

    func space(holding pane: String) -> SpaceInfo? {
        spaces.first { $0.root.panes.contains { $0.id == pane } }
    }

    /// The daemon answered: drop the overrides of every divider whose ratio it has heard.
    private func settle() {
        if paneDrag?.committed == true { withAnimation(GridMotion.layout) { paneDrag = nil } }
        if pendingNewSpace != nil, layoutError != nil { pendingNewSpace = nil }
        for split in settling { dragRatios[split] = nil }
        settling = []
    }

    /// A terminal or an agent pane took keyboard focus (the person clicked into it): tell the
    /// daemon, unless it already has that pane focused. A pane whose space no window shows can't have
    /// been clicked: it is a hidden terminal still holding the first responder, so it is ignored.
    func paneFocused(pane: String) {
        guard let space = space(holding: pane) else { return }
        guard shownSpaces.contains(space.id) else {
            Log.info("pane focus \(pane): ignored, space \(space.id) is not shown in any window")
            return
        }
        guard space.focus != pane || space.id != active else { return }
        select(pane: pane)
    }

    /// Show a pane's space, focus the pane, and move the keyboard to it: a click in the pane.
    func select(pane: String) {
        guard space(holding: pane) != nil else { return }
        focusKey = SurfacePlan.key(pane: pane, in: spaces)
        focus(pane: pane)
    }

    // MARK: Sessions — every agent pane

    /// Stream a session's conversation to this window; `session_attached` resets its transcript.
    /// The daemon has just said something about `session`: when its attach was refused earlier, the
    /// session exists now, so attach again.
    private func attachAgainIfRefused(_ session: String) {
        guard refusedAttach.remove(session) != nil, let chat = chats[session] else { return }
        Log.info("session \(session) is there now; attaching again")
        attach(chat)
    }

    private func attach(_ chat: SessionChat) {
        let req = client.req()
        sessionReqs[req] = chat.name
        attachReqs.insert(req)
        Log.info("attaching to session \(chat.name)")
        client.send(.sessionAttach(req: req, session: chat.name))
    }

    /// Take down the notice over `chat`: the banner's close button. The daemon forgets it and tells
    /// every client, this one included, so the banner goes when the daemon says so.
    func dismissNotice(_ chat: SessionChat) {
        guard chat.notice != nil, connection == .connected else { return }
        Log.info("dismiss notice over \(chat.name)")
        client.send(.sessionNoticeDismiss(req: nil, session: chat.name))
    }

    /// Send a session's draft, the text in its composer. The draft clears once the message goes
    /// out; when it can't go out now, nothing is sent and the draft stays in the box.
    func send(_ chat: SessionChat) {
        let text = chat.draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        guard connection == .connected else {
            chat.error = "Not connected to wheeld; your message is still in the box."
            return
        }
        chat.error = nil
        // Sending is going back to the conversation's end: the person wants to see what they sent
        // and what comes back, wherever they had scrolled.
        chat.pinned = true
        let req = client.req()
        sessionReqs[req] = chat.name
        // Sized, never the text, as the daemon logs it.
        Log.info("send to \(chat.name), \(text.count) chars")
        client.send(.sessionSend(req: req, session: chat.name, text: text))
        chat.draft = ""
    }

    func cancel(_ chat: SessionChat) {
        Log.info("cancel \(chat.name)")
        client.send(.sessionCancel(req: client.req(), session: chat.name))
    }

    /// Change one of a session's config options. The control redraws when the daemon broadcasts
    /// the agent's new list, never from the click, so what it shows is always what the agent took.
    func setConfig(_ chat: SessionChat, _ option: ConfigOption, to value: String) {
        guard connection == .connected else {
            chat.error = "Not connected to wheeld."
            return
        }
        chat.error = nil
        let req = client.req()
        sessionReqs[req] = chat.name
        Log.info("set \(chat.name) \(option.id) to \(value)")
        client.send(.sessionConfigSet(req: req, session: chat.name, config: option.id, value: value))
    }

    /// Ask the daemon for the usage board; it answers `usage`. `dueOnly` reads the endpoint only
    /// when the last read is older than the daemon's interval.
    func askUsage(dueOnly: Bool) {
        guard connection == .connected else { return }
        if !dueOnly { usage.refreshStarted() }
        Log.info("usage ask dueOnly=\(dueOnly)")
        client.send(.usage(req: client.req(), dueOnly: dueOnly))
    }

    /// Pin or unpin a usage row (`<agent>:<limit>`) on the strip: write the `usage.pins` setting
    /// machine-wide; the daemon's `usageChanged` brings the new board back.
    func togglePin(_ row: String) {
        guard connection == .connected else { return }
        let value = usage.board.pins(toggling: row)
        Log.info("usage pin \(row) -> usage.pins = \(value)")
        client.send(.settingSet(req: nil, key: "usage.pins", value: value, space: nil, machine: true))
    }

    /// Show or hide an agent's usage: write the `usage.agents` setting machine-wide.
    func toggleUsageAgent(_ agent: String) {
        guard connection == .connected else { return }
        let value = usage.board.agentsSetting(toggling: agent)
        Log.info("usage agent \(agent) -> usage.agents = \(value)")
        client.send(.settingSet(req: nil, key: "usage.agents", value: value, space: nil, machine: true))
    }

    /// Allow or deny a waiting request; the daemon broadcasts the result to every client.
    func answer(_ perm: PermInfo, _ choice: String) {
        Log.info("answering \(perm.id): \(choice)")
        client.send(.permAnswer(req: client.req(), id: perm.id, choice: choice))
    }

    // MARK: Settings — the Settings window and the debug endpoint call these.

    /// The window came up: the Project tab starts on the space being shown.
    func settingsAppeared() {
        Log.info("settings window opened")
        settings.open = true
        settings.project = active ?? spaces.first?.id
        settings.commandLine.refresh()
        askSettings()
        askRemote()
        askLogShipping()
        // wheeld reports the shipping state only when asked; the open window keeps it current.
        logShippingRefresh?.invalidate()
        logShippingRefresh = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.askLogShipping() }
        }
    }

    /// Ask wheeld for the folders a home field's agent could use and what the field's folder holds.
    func askHomes(_ field: SettingsModel.Field) {
        guard let agent = SettingsModel.homeAgent(field.key), connection == .connected else { return }
        let text = settings.text(field)
        let req = client.req()
        settings.homeReqs[req] = (field, text)
        client.send(.agentHomes(req: req, agent: agent, path: text))
    }

    /// Ask wheeld for its status, for the log shipping state Settings shows beside the push URL.
    func askLogShipping() {
        guard connection == .connected else { return }
        let req = client.req()
        settings.statusReq = req
        client.send(.status(req: req))
    }

    /// The log shipping state as wheeld reported it; a change of state or error is logged.
    private func showLogShipping(_ logs: LogShipping?) {
        let was = settings.logShipping
        settings.logShipping = logs
        guard let logs, logs.state != was?.state || logs.error != was?.error || logs.url != was?.url else { return }
        Log.info("log shipping: \(logs.state) to \(logs.url.isEmpty ? "no URL" : logs.url), \(logs.waitingBytes) bytes waiting\(logs.error.map { ", error: \($0)" } ?? "")")
    }

    /// Ask how remote clients reach the daemon; it answers `remote`.
    func askRemote() {
        guard connection == .connected else { return }
        Log.info("remote info ask")
        client.send(.remoteInfo(req: client.req()))
        client.send(.deviceList(req: client.req()))
    }

    /// Ask wheeld for a one-time pairing token; the Devices tab draws the code once it arrives.
    func requestPairingCode() {
        guard connection == .connected else { return }
        let req = client.req()
        pairingReq = req
        Log.info("pairing code asked for")
        client.send(.pairingIssue(req: req))
    }

    /// Tell wheeld to refuse these pairing tokens: the window or copy that showed them is gone.
    /// Ones that cannot be sent now go when the connection is back.
    func cancelPairing(tokens: [String]) {
        for token in tokens { settings.devices.cancelled(token: token) }
        cancelsToSend += tokens
        sendPairingCancels()
    }

    func sendPairingCancels() {
        guard connection == .connected else { return }
        for token in cancelsToSend {
            Log.info("pairing token cancelled")
            client.send(.pairingCancel(req: client.req(), token: token))
        }
        cancelsToSend = []
    }

    func revoke(device: String) {
        guard connection == .connected else { return }
        Log.info("revoking device \(device)")
        client.send(.deviceRevoke(req: client.req(), id: device))
    }

    func settingsDisappeared() {
        Log.info("settings window closed")
        settings.open = false
        logShippingRefresh?.invalidate()
        logShippingRefresh = nil
    }

    /// The Project tab's list: edit this space's `wheel.toml`.
    func selectSettingsProject(_ id: String) {
        guard id != settings.project else { return }
        Log.info("settings: project \(id)")
        settings.project = id
        askSettings()
    }

    /// The chosen project gone from the layout: the space being shown, else the first, else none.
    private func keepSettingsProject() {
        guard settings.open else { return }
        if let project = settings.project, spaces.contains(where: { $0.id == project }) { return }
        // No spaces and none chosen: the machine view it holds is still the one to show.
        if settings.project == nil, spaces.isEmpty { return }
        settings.project = active ?? spaces.first?.id
        askSettings()
    }

    /// What the links in a chat reply know and do: the layout is read from this model, requests go
    /// over its client, and a click that opens a pane goes through the same layout ops as the CLI's.
    lazy var links: ChatLinkResolver = {
        let resolver = ChatLinkResolver()
        resolver.locate = { [unowned self] session in
            ChatLinks.place(ofSession: session, in: spaces).map { .init(space: $0.space, cwd: $0.cwd, pane: $0.pane) }
        }
        resolver.paneChip = { [unowned self] id in ChatLinks.chip(for: id, in: spaces) }
        resolver.request = { [unowned self] make in
            guard connection == .connected else { return nil }
            let req = client.req()
            client.send(make(req))
            return req
        }
        resolver.perform = { [unowned self] action, session in performChatLink(action, session: session) }
        return resolver
    }()

    private func performChatLink(_ action: ChatLinkResolver.Action, session: String) {
        Log.info("chat link from \(session): \(action)")
        func beside(url: String? = nil, artifact: String? = nil) {
            guard let place = links.locate(session) else { return }
            zoomedPanes[place.space] = nil
            sendLayoutOp { .paneOpen(req: $0, pane: place.pane, dir: .right, url: url, artifact: artifact) }
        }
        switch action {
        case let .focusPane(id): select(pane: id)
        case let .openPage(url): beside(url: url)
        case let .openArtifact(name): beside(artifact: name)
        case let .openFile(path):
            let expanded = (path as NSString).expandingTildeInPath
            beside(url: URL(fileURLWithPath: expanded).absoluteString)
        case let .openInSystemBrowser(url):
            if let url = URL(string: url) { NSWorkspace.shared.open(url) }
        case let .runCommand(command):
            guard let place = links.locate(session) else { return }
            zoomedPanes[place.space] = nil
            sendLayoutOp { .paneRun(req: $0, pane: place.pane, command: command) }
        case let .copyText(text):
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        }
    }

    /// The appearance of the chat showing `session`, from its space's resolved `chat.*` settings.
    func appearance(forSession session: String) -> ChatAppearance {
        appearances.appearance(forSession: session, in: spaces)
    }

    /// Ask for the settings of every space an appearance is not held for. A chat reads the
    /// values resolved for its own space, so the project's file wins in it and in no other.
    private func askAppearances() {
        guard connection == .connected else { return }
        for space in appearances.missing(in: spaces) where !appearanceReqs.values.contains(space) {
            let req = client.req()
            appearanceReqs[req] = space
            client.send(.settingsLs(req: req, space: space))
        }
    }

    /// Ask for the settings of every space but `except`: a machine-wide write changed what each
    /// one falls back to, and a space's own write changed that space.
    private func refreshAppearances(except: String?) {
        guard connection == .connected else { return }
        for space in spaces.map(\.id) where space != except {
            let req = client.req()
            appearanceReqs[req] = space
            client.send(.settingsLs(req: req, space: space))
        }
    }

    /// Ask for the settings once at connect, for `status.dots`; a later change arrives as a broadcast.
    private func askStatusDots() {
        let req = client.req()
        statusDotsReq = req
        client.send(.settingsLs(req: req, space: nil))
    }

    func paneStatus(_ pane: String) -> PaneStatus? { spaces.status(ofPane: pane) }

    /// Show or hide the dot for one rank: write `status.dots` machine-wide.
    func toggleStatusDot(_ rank: StatusRank) {
        guard connection == .connected else { return }
        var ranks = statusDots
        if ranks.contains(rank) { ranks.remove(rank) } else { ranks.insert(rank) }
        let value = StatusRank.setting(ranks)
        Log.info("status dot \(rank.rawValue) -> status.dots = \(value)")
        client.send(.settingSet(req: nil, key: "status.dots", value: value, space: nil, machine: true))
    }

    /// Ask for every setting as it resolves for the chosen project. Each carries this machine's
    /// value too, so one list serves both tabs; with no project it is the machine-wide view.
    func askSettings() {
        guard connection == .connected else { return }
        let req = client.req()
        settings.listReq = req
        settings.asking(for: settings.project)
        Log.info("settings ls for \(settings.project ?? "no space")")
        client.send(.settingsLs(req: req, space: settings.project))
    }

    /// Return in a field: write what was typed into that layer's file, or remove the key from it
    /// when the field was emptied. A field holding what its file sets sends nothing.
    func saveSetting(_ field: SettingsModel.Field) {
        guard settings.dirty(field), let text = settings.drafts[field] else { return }
        if text.trimmingCharacters(in: .whitespaces).isEmpty {
            clearSetting(field)
        } else {
            change(field, "set \(field.key) = \(text)") { .settingSet(req: $0, key: field.key, value: text, space: self.settings.project, machine: field.layer == .global) }
        }
    }

    /// Write `text` into the field's file, with no draft: a control that already holds its value.
    func writeSetting(_ field: SettingsModel.Field, _ text: String) {
        change(field, "set \(field.key) = \(text)") { .settingSet(req: $0, key: field.key, value: text, space: self.settings.project, machine: field.layer == .global) }
    }

    /// The clear button: remove the key from that layer's file, so the layer below is in force.
    func clearSetting(_ field: SettingsModel.Field) {
        change(field, "unset \(field.key)") { .settingUnset(req: $0, key: field.key, space: self.settings.project, machine: field.layer == .global) }
    }

    private func change(_ field: SettingsModel.Field, _ what: String, _ make: (UInt64) -> ClientMsg) {
        guard connection == .connected else {
            settings.refused(field, "Not connected to wheeld; your change is still in the field.")
            return
        }
        let req = client.req()
        settings.changeReqs[req] = field
        settings.sending(field)
        Log.info("setting \(what) in \(field.layer == .global ? "this machine's" : "\(settings.project ?? "?")'s") wheel.toml")
        client.send(make(req))
    }

    /// A list of settings: the answer to a change or a list, or another client's change. A
    /// broadcast resolved for a space other than the chosen project still may have changed this
    /// machine's value, which every list carries, so the window asks for its own again.
    private func received(settings list: [SettingInfo], for space: String?, req: UInt64?) {
        if let dots = list.first(where: { $0.key == "status.dots" }) {
            let ranks = StatusRank.listed(dots.value)
            if ranks != statusDots { Log.info("status.dots = \(dots.value)") }
            statusDots = ranks
        }
        if let keybind = list.first(where: { $0.key == "keybind" }), keybind.value != keybindSetting {
            keybindSetting = keybind.value
            rebuildKeymap()
        }
        if let req, appearanceReqs.removeValue(forKey: req) != nil {
            storeAppearance(list, for: space)
            return
        }
        if req == nil {
            storeAppearance(list, for: space)
            refreshAppearances(except: space)
            // The home a chat writes beside its composer comes from the agent list, which a
            // change to `agent.<name>.home` leaves stale until status is asked again.
            let moved = agents.contains { agent in
                list.first { $0.key == "agent.\(agent.name).home" }.map { $0.value != (agent.home ?? "") } ?? false
            }
            if moved { client.send(.status(req: client.req())) }
        }
        if let req, req == statusDotsReq {
            statusDotsReq = nil
            return
        }
        Log.info("settings for \(space ?? "no space"): \(list.map { "\($0.key)=\($0.value)(\($0.origin.rawValue)) machine=\($0.machine ?? "-") project=\($0.project ?? "-")" }.joined(separator: " "))")
        if let req, let field = settings.changeReqs.removeValue(forKey: req) {
            settings.changed(field)
            // Answered for the project the change was made on; if the tab has moved on since, its
            // own list is already on the way.
            if space == settings.project { settings.show(list, for: space) }
        } else if let req, req == settings.listReq {
            settings.listReq = nil
            settings.show(list, for: space)
        } else if req == nil {
            if space == settings.space, settings.loaded {
                settings.show(list, for: space)
            } else if settings.open {
                askSettings()
            }
        }
    }

    private func storeAppearance(_ list: [SettingInfo], for space: String?) {
        if let space { restyleTerminals(of: space, from: list) }
        var held = appearances
        held.store(list, for: space)
        guard held != appearances else { return }
        appearances = held
        if let space, let look = held.bySpace[space] {
            Log.info("chat appearance for \(space): \(look.family.rawValue) \(look.size)pt, line \(look.lineHeight), paragraphs \(look.paragraphSpacing)pt, width \(look.maxWidth.map { "\(Int($0))" } ?? "none"), \(look.density.rawValue)")
        }
    }

    /// A space's `terminal.*` settings changed: restyle its terminals' controller in place, so the
    /// shells keep running and every surface redraws.
    private func restyleTerminals(of space: String, from list: [SettingInfo]) {
        let look = TerminalAppearance(settings: list)
        guard terminalLooks[space] != look else { return }
        terminalLooks[space] = look
        Log.info("terminal appearance for \(space): \(look.fontFamily.isEmpty ? "own font" : look.fontFamily) \(look.fontSize)pt, line \(look.lineHeight), \(look.theme.rawValue), cursor \(look.cursor.rawValue)\(look.cursorBlink ? " blinking" : "")")
        guard let controller = terminalControllers[space] else { return }
        let a = controller.setTerminalConfiguration(Self.terminalConfiguration(look))
        let b = controller.setTheme(Self.terminalTheme(look))
        Log.info("terminal controller for \(space) restyled (config \(a), theme \(b))")
    }

    // MARK: Web panes

    /// Load a web pane's page, once the token for its address is known.
    private func open(_ web: WebPane) {
        guard pageDaemon.token != nil, let url = WebPanes.pageURL(pane: web.pane) else { return }
        web.load(url)
    }

    // MARK: Browser panes

    /// A browser pane landed on a new page: the saved layout follows it. The report carries no
    /// request, so the daemon refusing a page no pane may be on (a `data:` document) only logs.
    private func reportPage(_ pane: String, _ url: String) {
        client.send(.paneBrowserUrl(req: nil, pane: pane, url: url))
    }

    /// Ask the daemon what text typed into a browser pane's address field means.
    private func openTyped(_ pane: String, _ text: String) {
        guard connection == .connected else {
            browserPanes[pane]?.refused(typed: text, "Not connected to wheeld, which says what an address means.")
            return
        }
        let req = client.req()
        openReqs[req] = (pane, text)
        client.send(.paneBrowserOpen(req: req, pane: pane, text: text))
    }

    /// Send a browser pane to the `browser.homepage` setting as the daemon resolves it for the
    /// pane's space: the kind again with no page, which is what opening one without a page does.
    func goHome(_ pane: String) {
        browserPanes[pane]?.clearFailure()
        sendLayoutOp { .paneSetKind(req: $0, pane: pane, kind: .browser) }
    }

    // MARK: Terminals

    private func attach(_ terminal: PaneTerminal) {
        Log.info("attaching to \(terminal.term)")
        client.send(.termAttach(req: client.req(), id: terminal.term))
        if let size = terminal.size { client.send(.termResize(req: nil, id: terminal.term, cols: size.cols, rows: size.rows)) }
    }

    fileprivate func typed(_ term: String, _ data: Data) {
        guard terminals[term]?.attached == true else { return }
        client.send(.termInput(req: nil, id: term, data: data))
    }

    fileprivate func resized(_ term: String, _ vp: InMemoryTerminalViewport) {
        guard vp.columns > 0, vp.rows > 0, let terminal = terminals[term] else { return }
        terminal.reported = vp
        settle(terminal)
    }

    private func logSurface(_ term: String, _ line: String) {
        guard lastSurfaceLog[term] != line else { return }
        lastSurfaceLog[term] = line
        Log.info(line)
    }

    /// Send the surface's grid to the daemon once it fits the pane's laid-out size. A new surface
    /// first reports a default 800x600 frame minus its padding, before SwiftUI lays it out in the
    /// pane; the daemon would resize the shell to that grid and then again to the real one.
    /// Called when the surface reports and when the pane's layout changes, whichever comes last.
    func settle(_ terminal: PaneTerminal) {
        guard let vp = terminal.reported else { return }
        let cols = Int(vp.columns), rows = Int(vp.rows)
        guard let layout = terminal.layout, vp.widthPixels > 0, vp.heightPixels > 0 else {
            logSurface(terminal.term, "surface \(terminal.term) \(cols)x\(rows): held until the pane is laid out")
            return
        }
        let slack = PaneTerminal.paddingPoints * layout.scale
        let dw = layout.size.width * layout.scale - CGFloat(vp.widthPixels)
        let dh = layout.size.height * layout.scale - CGFloat(vp.heightPixels)
        let fits = (0...slack + CGFloat(vp.cellWidthPixels)).contains(dw) && (0...slack + CGFloat(vp.cellHeightPixels)).contains(dh)
        logSurface(terminal.term, "surface \(terminal.term) \(cols)x\(rows) at \(vp.widthPixels)x\(vp.heightPixels)px in pane \(layout.size.width)x\(layout.size.height)pt @\(layout.scale)x: \(fits ? "sent" : "held until laid out")")
        guard fits else { return }
        if let size = terminal.size, size == (cols, rows) { return }
        terminal.size = (cols, rows)
        GridMetrics.shared.resizeSent()
        client.send(.termResize(req: nil, id: terminal.term, cols: cols, rows: rows))
    }
}

/// One terminal pane's Ghostty surface and the in-memory session feeding it the daemon's bytes.
@MainActor
final class PaneTerminal: ObservableObject {
    let term: String
    /// The space whose controller styles this surface.
    let space: String?
    let view: TerminalViewState
    let session: InMemoryTerminalSession
    /// The daemon is streaming this terminal to this connection.
    @Published var attached = false
    /// The grid size last sent to the daemon.
    var size: (cols: Int, rows: Int)?
    /// The pane's laid-out size in points and the display scale, from SwiftUI.
    var layout: (size: CGSize, scale: CGFloat)?
    /// The surface's latest grid, held until `layout` says the surface has its final frame.
    var reported: InMemoryTerminalViewport?
    /// The most the pane exceeds its surface by on one axis, besides the cell remainder: the
    /// surface's padding, 20x16 pt measured.
    static let paddingPoints: CGFloat = 24

    init(term: String, space: String?, controller: TerminalController, model: AppModel) {
        self.term = term
        self.space = space
        view = TerminalViewState(controller: controller)
        // Only columns and rows reach the daemon, so a divider drag's pixel-only updates are noise.
        session = InMemoryTerminalSession(
            write: { [weak model] data in
                Task { @MainActor in model?.typed(term, data) }
            },
            resize: { [weak model] vp in
                Task { @MainActor in model?.resized(term, vp) }
            },
            suppressesPixelOnlyResizes: true
        )
        view.configuration = TerminalSurfaceOptions(backend: .inMemory(session))
    }

    /// The terminal's visible text, for the debug state endpoint.
    var viewportText: String? { session.readViewportText() }
}

extension AppModel {
    /// Send a layout op whose failure shows above the grid, as the grid's own ops do.
    func sendLayoutOp(_ msg: ClientMsg, req: UInt64) {
        layoutReqs.insert(req)
        client.send(msg)
    }
}

/// What a drag changes in the grid: the ratios of dividers under the pointer and the pane being
/// dragged by its header. Only `SpaceGrid` observes it, so a drag redraws the grid and not each pane.
@MainActor
final class GridDragState: ObservableObject {
    @Published var ratios: [String: Double] = [:]
    @Published var pane: AppModel.PaneDrag?
}
