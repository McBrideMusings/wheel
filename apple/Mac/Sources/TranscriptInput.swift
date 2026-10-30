import AppKit
import SwiftUI
import WheelKit
import WheelUI

/// The Mac's side of a transcript: the scroll-wheel and key monitor that tells a person's scroll from
/// layout settling, the arrow, Home and End stepping the table view ignores, and the cross-row text
/// selection. `TranscriptView` reaches them through the environment's `TranscriptPlatform`.
@MainActor
enum TranscriptInput {
    static let platform = TranscriptPlatform(
        appear: { chat in
            Log.info("transcript \(chat.name): watching the wheel, keys and selection")
            watchWheel(chat)
            chat.selection.watch(chat)
        },
        disappear: { chat in
            Log.info("transcript \(chat.name): stopped watching")
            if let monitor = chat.wheelMonitor { NSEvent.removeMonitor(monitor) }
            chat.wheelMonitor = nil
            chat.selection.stop()
        },
        proseHighlight: { prose, id, style, chat in
            AnyView(ProseHighlight(selection: chat.selection, chat: chat, id: id, prose: prose, style: style))
        },
        itemHighlight: { item, chat in
            AnyView(ItemHighlight(selection: chat.selection, chat: chat, item: item))
        }
    )

    /// The keys that scroll a transcript: Page Up/Down, Home, End and the arrows.
    private static let scrollKeys = Set(ScrollKey.allCases.map(\.keyCode))

    /// The keys the table view ignores: Home, End, Up and Down.
    private static let stepKeys = Set(ScrollKey.allCases.filter(\.tableIgnoresIt).map(\.keyCode))

    /// Moves the transcript's scroll view for a key the table ignores: Home to the top, End to
    /// the end, an arrow by one line. Marked as a person's scroll while it lands, so the view
    /// leaves the end or follows it again the way it does for a wheel.
    private static func step(_ scroll: NSScrollView, key: UInt16, chat: SessionChat) {
        guard let document = scroll.documentView else { return }
        let clip = scroll.contentView
        let line: CGFloat = 24
        let bottom = max(0, document.frame.height - clip.bounds.height)
        let target: CGFloat
        switch key {
        case ScrollKey.home.keyCode: target = 0
        case ScrollKey.end.keyCode: target = bottom
        case ScrollKey.up.keyCode: target = clip.bounds.origin.y - line
        default: target = clip.bounds.origin.y + line
        }
        let y = min(max(0, target), bottom)
        if key == ScrollKey.end.keyCode || (key == ScrollKey.down.keyCode && bottom - y < 24) {
            Log.info("transcript \(chat.name): key \(key) to the end")
            chat.endRequest += 1
            return
        }
        chat.userScrolling = true
        clip.scroll(to: NSPoint(x: clip.bounds.origin.x, y: y))
        scroll.reflectScrolledClipView(clip)
        DispatchQueue.main.async { chat.userScrolling = false }
        Log.info("transcript \(chat.name): key \(key) scrolled to \(Int(y)) of \(Int(bottom))")
    }

    /// Watches the wheel and the keys for this transcript. Stamps `chat.wheelAt` whenever a person's
    /// scroll input reaches it: a scroll-wheel event inside its frame, or a scrolling key (Page Up/Down,
    /// Home, End, the arrows) while a view inside the transcript, not a text editor, has the keyboard.
    /// Keys produce no wheel event, and a text editor takes them for its own caret. A printable key
    /// goes to the focused chat's composer from the app-level monitor (`typeIntoFocusedComposer`).
    private static func watchWheel(_ chat: SessionChat) {
        if let monitor = chat.wheelMonitor { NSEvent.removeMonitor(monitor) }
        chat.wheelMonitor = NSEvent.addLocalMonitorForEvents(matching: [.scrollWheel, .keyDown]) { event in
            guard let window = event.window else { return event }
            switch event.type {
            case .scrollWheel:
                if let height = window.contentView?.bounds.height {
                    let point = CGPoint(x: event.locationInWindow.x, y: height - event.locationInWindow.y)
                    if chat.frameInWindow.contains(point) { chat.wheelAt = Date() }
                }
            case .keyDown:
                // Only when the keyboard sits in a view inside this transcript's frame, not in a
                // text editor, a terminal or a button elsewhere.
                if scrollKeys.contains(event.keyCode), !(window.firstResponder is NSText),
                   let view = window.firstResponder as? NSView, let height = window.contentView?.bounds.height {
                    let box = view.convert(view.bounds, to: nil)
                    let center = CGPoint(x: box.midX, y: height - box.midY)
                    if chat.frameInWindow.contains(center) {
                        chat.wheelAt = Date()
                        // Page Up/Down scroll the table natively; Home, End and the arrows do
                        // nothing in it, so the transcript moves its own scroll view.
                        if let scroll = view.enclosingScrollView, stepKeys.contains(event.keyCode) {
                            step(scroll, key: event.keyCode, chat: chat)
                            return nil
                        }
                    }
                }
            default:
                break
            }
            return event
        }
    }
}

extension AppModel {
    /// Whether `responder` keeps printable keys for itself: an editable text view or field editor, or
    /// a terminal, web or browser pane's view. A selectable but read-only text, such as a clicked
    /// reply, does not.
    private func holdsTextKeys(_ responder: NSResponder?) -> Bool {
        if let text = responder as? NSText { return text.isEditable }
        if terminals.values.contains(where: { $0.view.isFocused }) { return true }
        guard let view = responder as? NSView else { return false }
        return (webPanes.values.map(\.view) + browserPanes.values.map(\.view) as [NSView]).contains { view === $0 || view.isDescendant(of: $0) }
    }

    /// A printable key without ⌘, ⌃, ⌥ or fn, typed anywhere in the focused Wheel window while its
    /// focused pane is an agent pane showing its composer and no editable text input has the
    /// keyboard, joins that composer's draft and moves the keyboard there. False leaves the key to
    /// whoever has it: bound keys, scrolling keys, ⌘C, a text field, a terminal or a page.
    func typeIntoFocusedComposer(_ event: NSEvent) -> Bool {
        guard let window = event.window, window.isWheelWindow, window.attachedSheet == nil,
              commandPalette == nil, !shortcutsOpen,
              event.modifierFlags.intersection(.deviceIndependentFlagsMask).isDisjoint(with: [.command, .control, .option, .function]),
              let characters = event.characters, SessionChat.isTyped(characters),
              let pane = activeSpace?.focus, case let .agent(session, _, _)? = kind(of: pane),
              let chat = chats[session], chat.composerShown,
              !holdsTextKeys(window.firstResponder)
        else { return false }
        Log.info("key typed in \(pane): \(characters.count) char(s) to the composer, first responder \(window.firstResponder.map { String(describing: type(of: $0)) } ?? "none")")
        chat.typeIntoComposer(characters)
        return true
    }
}

/// An agent's transcript on the Mac: `TranscriptView` with the Mac's input handling, fed the
/// permission requests the model holds.
struct AgentTranscript: View {
    var model: AppModel
    @ObservedObject var chat: SessionChat
    @Environment(\.spaceShown) private var spaceShown
    let empty: String
    var agent: String? = nil

    private var intro: String? {
        guard let agent, model.appearance(forSession: chat.name).home == .transcript else { return nil }
        return model.homeIntro(session: chat.name, agent: agent)
    }

    @ViewBuilder var body: some View {
        if spaceShown { transcript } else { Color.clear }
    }

    private var transcript: some View {
        TranscriptView(
            chat: chat,
            empty: empty,
            permissions: PermissionQueue(
                waiting: Set(model.perms.filter { $0.session == chat.name }.compactMap(\.toolCall)),
                places: Dictionary(
                    model.perms.compactMap { perm in perm.toolCall.map { ($0, model.position(of: perm)) } },
                    uniquingKeysWith: { first, _ in first }),
                count: model.perms.count),
            look: model.appearance(forSession: chat.name),
            intro: intro
        )
        .environment(\.transcriptPlatform, TranscriptInput.platform)
        .environment(\.chatLinks, model.links)
    }
}
