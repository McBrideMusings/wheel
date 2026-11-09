import Foundation
import Testing
@testable import WheelKit

struct WindowLayoutTests {
    private func sessions(_ groups: [[String]]) -> [SessionInfo] {
        groups.enumerated().map { SessionInfo(id: "n\($0.offset + 1)", name: "session \($0.offset + 1)", spaces: $0.element) }
    }

    /// Two sessions, `n1` holding s1 s2 and `n2` holding s3, with one window on each.
    private func layout() -> (WindowLayout, String, String) {
        var layout = WindowLayout()
        layout.newWindow()
        layout.reconcile(sessions([["s1", "s2"], ["s3"]]))
        let first = layout.windows[0].id
        let second = layout.newWindow(session: "n2")
        return (layout, first, second)
    }

    /// The launch window has no session; the first report points it at the first session and its first space.
    @Test func theLaunchWindowShowsTheFirstSession() {
        var layout = WindowLayout()
        layout.newWindow()
        layout.reconcile(sessions([["s1", "s2"], ["s3"]]))
        #expect(layout.windows.count == 1)
        #expect(layout.windows[0].session == "n1")
        #expect(layout.spaces(of: layout.windows[0]) == ["s1", "s2"])
        #expect(layout.windows[0].active == "s1")
        #expect(layout.backgroundSessions.map(\.id) == ["n2"])
        #expect(layout.background == ["s3"])
    }

    /// A report that lists no session (wheeld still loading) closes and changes nothing.
    @Test func aReportWithNoSessionsLeavesTheWindowsAlone() {
        var (layout, first, _) = layout()
        layout.reconcile([])
        #expect(layout.windows.count == 2)
        #expect(layout.window(first)?.session == "n1")
    }

    /// A report opens no window: a person who closed every window keeps none.
    @Test func aReportOpensNoWindow() {
        var layout = WindowLayout()
        layout.reconcile(sessions([["s1"], ["s2"]]))
        #expect(layout.windows.isEmpty)
        layout.reconcile(sessions([["s1"], ["s2"], ["s3"]]))
        #expect(layout.windows.isEmpty)
    }

    /// A window opened before wheeld reported takes the first session when one arrives.
    @Test func aWindowWithNoSessionTakesTheFirst() {
        var layout = WindowLayout(windows: [WindowRecord()])
        #expect(layout.windows[0].session == nil)
        layout.reconcile(sessions([["s1"], ["s2"]]))
        #expect(layout.windows.count == 1)
        #expect(layout.windows[0].session == "n1")
    }

    /// A little window left ownerless takes an owner once a window shows its session.
    @Test func anOwnerlessLittleWindowTakesAnOwnerWhenAWindowShowsItsSession() throws {
        var (layout, _, second) = layout()
        layout.close(window: second)
        let opened = layout.openLittle("s3")
        let little = try #require(opened)
        #expect(layout.little(little)?.owner == nil)
        layout.show(session: "n2", in: layout.windows[0].id)
        #expect(layout.little(little)?.owner == layout.windows[0].id)
    }

    /// Building or collapsing a layout before any report keeps each window's last-space memory.
    @Test func collapseBeforeAnyReportKeepsTheLastSpaces() {
        var layout = WindowLayout(windows: [WindowRecord(session: "n1", lastSpace: ["n1": "s2"])])
        layout.collapse()
        #expect(layout.windows[0].lastSpace == ["n1": "s2"])
    }

    /// Two windows may show the same session, each its own space of it.
    @Test func twoWindowsShowOneSessionEachItsOwnSpace() {
        var (layout, first, _) = layout()
        let second = layout.newWindow(session: "n1", space: "s2")
        #expect(layout.window(first)?.active == "s1")
        #expect(layout.window(second)?.active == "s2")
        #expect(layout.windows(showing: "n1").count == 2)
        #expect(layout.backgroundSessions.isEmpty)
        layout.close(window: second)
        #expect(layout.backgroundSessions.isEmpty)
        layout.close(window: first)
        #expect(layout.backgroundSessions.map(\.id) == ["n1"])
        #expect(layout.background == ["s1", "s2"])
    }

    /// A session no window shows is background, and showing it in a window ends that.
    @Test func closingTheLastWindowOfASessionBackgroundsIt() {
        var (layout, first, second) = layout()
        layout.close(window: second)
        #expect(layout.windows.map(\.id) == [first])
        #expect(layout.backgroundSessions.map(\.id) == ["n2"])
        let shown = layout.show(session: "n2", in: first)
        #expect(shown)
        #expect(layout.window(first)?.active == "s3")
        #expect(layout.backgroundSessions.map(\.id) == ["n1"])
    }

    /// Switching a window's session returns to the space it last showed there.
    @Test func switchingSessionsReturnsToTheLastSpace() {
        var (layout, first, _) = layout()
        layout.show("s2", in: first)
        layout.show(session: "n2", in: first)
        #expect(layout.window(first)?.active == "s3")
        layout.show(session: "n1", in: first)
        #expect(layout.window(first)?.active == "s2")
        let refused = layout.show(session: "n9", in: first)
        #expect(!refused)
        #expect(layout.window(first)?.session == "n1")
    }

    /// Showing a space that is not in the window's session does nothing.
    @Test func showingASpaceOfAnotherSessionDoesNothing() {
        var (layout, first, _) = layout()
        layout.show("s3", in: first)
        #expect(layout.window(first)?.active == "s1")
    }

    /// A space that moved between sessions follows its session: the window showing the old one
    /// falls to a neighbour, the window showing the new one lists it.
    @Test func aMovedSpaceLeavesOneSessionsTabsForTheOthers() {
        var (layout, first, second) = layout()
        layout.show("s2", in: first)
        layout.reconcile(sessions([["s1"], ["s3", "s2"]]))
        #expect(layout.spaces(of: layout.window(first)!) == ["s1"])
        #expect(layout.window(first)?.active == "s1")
        #expect(layout.spaces(of: layout.window(second)!) == ["s3", "s2"])
    }

    /// A window whose session closed shows the first session.
    @Test func aWindowOfAClosedSessionShowsTheFirst() {
        var (layout, first, second) = layout()
        layout.reconcile(sessions([["s1", "s2"]]))
        #expect(layout.window(second)?.session == "n1")
        #expect(layout.window(first)?.session == "n1")
    }

    /// A new space joins its session's tabs without being shown anywhere.
    @Test func aNewSpaceJoinsItsSessionWithoutBeingShown() {
        var (layout, first, second) = layout()
        let appeared = layout.reconcile(sessions([["s1", "s2", "s4"], ["s3"]]))
        #expect(appeared == ["s4"])
        #expect(layout.spaces(of: layout.window(first)!) == ["s1", "s2", "s4"])
        #expect(layout.window(first)?.active == "s1")
        #expect(layout.window(second)?.active == "s3")
    }

    /// The first report names nothing as new.
    @Test func theFirstReportAppearsNothing() {
        var layout = WindowLayout()
        layout.newWindow()
        #expect(layout.reconcile(sessions([["s1"]])).isEmpty)
    }

    /// A saved layout round-trips; the sessions are wheeld's, so they are not saved.
    @Test func aSavedLayoutRoundTripsWithoutTheSessions() throws {
        var (saved, first, _) = layout()
        saved.rename(window: first, to: "Reviews")
        saved.show("s2", in: first)
        let back = try JSONDecoder().decode(WindowLayout.self, from: JSONEncoder().encode(saved))
        #expect(back.windows == saved.windows)
        #expect(back.sessions.isEmpty)
        #expect(back.window(first)?.name == "Reviews")
        #expect(back.window(first)?.session == "n1")
        #expect(back.window(first)?.active == "s2")
    }

    /// Reconciling after a restore keeps each window's session and drops a saved space that is gone.
    @Test func aRestoredLayoutKeepsItsSessionsAndFallsBackFromAGoneSpace() throws {
        var (saved, first, second) = layout()
        saved.show("s2", in: first)
        var back = try JSONDecoder().decode(WindowLayout.self, from: JSONEncoder().encode(saved))
        back.reconcile(sessions([["s1"], ["s3"]]))
        #expect(back.window(first)?.session == "n1")
        #expect(back.window(first)?.active == "s1")
        #expect(back.window(second)?.session == "n2")
    }

    /// A background space whose last pane closed is emptied; a new one, one in a shown session, and
    /// one whose pane only became empty are not.
    @Test func onlyABackgroundSpaceWhoseLastPaneClosedIsEmptied() {
        var layout = WindowLayout()
        layout.newWindow()
        layout.reconcile(sessions([["s1"], ["s2", "s3", "s4"]]))
        func space(_ id: String, _ pane: String, _ kind: PaneKind) -> SpaceInfo {
            SpaceInfo(id: id, name: id, cwd: "/tmp", root: .pane(id: pane, kind: kind), focus: pane)
        }
        let old = [space("s1", "p1", .terminal(term: "t1")), space("s2", "p2", .terminal(term: "t2")),
                   space("s3", "p3", .terminal(term: "t3"))]
        let new = [space("s1", "p11", .empty), space("s2", "p12", .empty),
                   space("s3", "p3", .empty), space("s4", "p4", .empty)]
        #expect(layout.emptied(from: old, to: new) == ["s2"])
    }

    /// Launching with restore off leaves one window, the focused one's, showing its session.
    @Test func collapseKeepsTheFocusedWindowAlone() {
        var (layout, _, second) = layout()
        layout.collapse()
        #expect(layout.windows.map(\.id) == [second])
        #expect(layout.window(second)?.session == "n2")
        #expect(layout.littles.isEmpty)
    }

    /// Tearing a space off opens a window on its session showing that space.
    @Test func tearOffOpensAWindowOnTheSpacesSessionAtTheSpace() throws {
        var (layout, first, _) = layout()
        let opened_torn = layout.tearOff("s2")
        let torn = try #require(opened_torn)
        #expect(layout.window(torn)?.session == "n1")
        #expect(layout.window(torn)?.active == "s2")
        #expect(layout.window(first)?.active == "s1")
        let none = layout.tearOff("s9")
        #expect(none == nil)
    }

    /// A little window keeps its space in its owner's tabs, and closing the owner leaves it open
    /// belonging to none, its space out of the background.
    @Test func aLittleWindowOutlivesItsOwnerAndKeepsItsSpaceOutOfTheBackground() throws {
        var (layout, first, _) = layout()
        let opened_little = layout.openLittle("s1")
        let little = try #require(opened_little)
        #expect(layout.little(little)?.owner == first)
        #expect(layout.spaces(of: layout.window(first)!).contains("s1"))
        layout.close(window: first)
        #expect(layout.little(little)?.owner == nil)
        #expect(layout.background == ["s2"])
        layout.closeLittle(little)
        #expect(layout.background == ["s1", "s2"])
    }

    /// A little window moves to another window showing its space when its owner's session no longer holds it.
    @Test func aLittleWindowFollowsItsSpaceToAnotherOwner() throws {
        var (layout, _, second) = layout()
        let opened_little = layout.openLittle("s1")
        let little = try #require(opened_little)
        layout.reconcile(sessions([["s2"], ["s3", "s1"]]))
        #expect(layout.little(little)?.owner == second)
    }

    /// A background space opens in an ownerless little window; a space no session holds opens nowhere.
    @Test func aBackgroundSpaceOpensInAnOwnerlessLittleWindow() throws {
        var (layout, _, second) = layout()
        layout.close(window: second)
        let opened_little = layout.openLittle("s3")
        let little = try #require(opened_little)
        #expect(layout.little(little)?.owner == nil)
        #expect(layout.background.isEmpty)
        let none = layout.openLittle("s9")
        let again = layout.openLittle("s3")
        #expect(none == nil)
        #expect(again == little)
    }

    /// A little window ends when its space is gone.
    @Test func aLittleWindowEndsWhenItsSpaceIsGone() throws {
        var (layout, _, _) = layout()
        let opened_little = layout.openLittle("s2")
        let little = try #require(opened_little)
        layout.reconcile(sessions([["s1"], ["s3"]]))
        #expect(layout.little(little) == nil)
    }

    @Test func aWindowIsTitledByItsNameOrItsActiveSpace() {
        var layout = WindowLayout()
        layout.newWindow()
        layout.reconcile(sessions([["s1"]]))
        let id = layout.windows[0].id
        let names = ["s1": "plex-db"]
        #expect(layout.title(of: layout.windows[0]) { names[$0] } == "plex-db")
        layout.rename(window: id, to: "  Reviews ")
        #expect(layout.title(of: layout.windows[0]) { names[$0] } == "Reviews")
        layout.rename(window: id, to: "   ")
        #expect(layout.title(of: layout.windows[0]) { names[$0] } == "plex-db")
    }

    /// A window's mode is its own: setting one leaves the other window alone, it is saved with the layout, and a window
    /// saved with no mode shows spaces.
    @Test func modeBelongsToOneWindowAndDefaultsToSpaces() throws {
        var (saved, first, second) = layout()
        saved.setMode(WindowMode.desk, in: first)
        #expect(saved.window(first)?.mode == "desk")
        #expect(saved.window(second)?.mode == "spaces")
        let back = try JSONDecoder().decode(WindowLayout.self, from: JSONEncoder().encode(saved))
        #expect(back.window(first)?.mode == "desk")
        let old = #"{"windows":[{"id":"w","session":"n1","sidebar":true}],"littles":[]}"#
        let legacy = try JSONDecoder().decode(WindowLayout.self, from: Data(old.utf8))
        #expect(legacy.windows[0].mode == "spaces")
        #expect(WindowMode.builtIn == ["spaces", "desk"])
    }

    /// A pin on a window or a little window is saved with the layout; a layout saved before pins existed reads as unpinned.
    @Test func pinsRoundTripAndDefaultOff() throws {
        var (saved, first, _) = layout()
        let opened = saved.openLittle("s1")
        let little = try #require(opened)
        saved.setPinned(true, window: first)
        saved.setPinned(true, little: little)
        let back = try JSONDecoder().decode(WindowLayout.self, from: JSONEncoder().encode(saved))
        #expect(back.window(first)?.pinned == true)
        #expect(back.little(little)?.pinned == true)
        #expect(back.windows.filter(\.pinned).count == 1)
        saved.setPinned(false, window: first)
        #expect(saved.window(first)?.pinned == false)
        let old = #"{"windows":[{"id":"w","session":"n1","sidebar":true}],"littles":[{"id":"l","space":"a"}]}"#
        let legacy = try JSONDecoder().decode(WindowLayout.self, from: Data(old.utf8))
        #expect(legacy.windows[0].pinned == false)
        #expect(legacy.littles[0].pinned == false)
    }
}
