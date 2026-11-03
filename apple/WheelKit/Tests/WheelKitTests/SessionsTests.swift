import Foundation
import Testing
@testable import WheelKit

struct SessionsTests {
    private let two = [SessionInfo(id: "n1", name: "wheel", spaces: ["s1", "s2", "s3"]), SessionInfo(id: "n2", name: "games", spaces: ["s4"])]

    @Test func decodesSessionsOnTheLayoutAndTheStatus() throws {
        let spaces = #"{"ev":"spaces","spaces":[],"sessions":[{"id":"n1","name":"wheel","spaces":["s1","s2"]}]}"#
        guard case let .spaces(_, sessions) = try ServerMsg.decode(spaces) else { Issue.record("wrong case"); return }
        #expect(sessions == [SessionInfo(id: "n1", name: "wheel", spaces: ["s1", "s2"])])
        let status = #"{"ev":"status","version":"x","log_path":"/l","uptime_secs":3,"terms":[],"agents":[],"sessions":[{"id":"n2","name":"games","spaces":[]}]}"#
        guard case let .status(_, _, _, _, _, _, _, _, _, _, _, _, _, listed) = try ServerMsg.decode(status) else { Issue.record("wrong case"); return }
        #expect(listed == [SessionInfo(id: "n2", name: "games", spaces: [])])
        guard case let .spaces(_, none) = try ServerMsg.decode(#"{"ev":"spaces","spaces":[]}"#) else { Issue.record("wrong case"); return }
        #expect(none.isEmpty, "a server that sends no sessions decodes as none")
    }

    @Test func encodesSessionOps() throws {
        #expect(try ClientMsg.sessionNew(req: 1, name: "games").json() == #"{"name":"games","op":"session_new","req":1}"#)
        #expect(try ClientMsg.sessionMove(req: 4, space: "s1", session: "n2").json() == #"{"op":"session_move","req":4,"session":"n2","space":"s1"}"#)
        #expect(try ClientMsg.spaceCreate(req: 5, cwd: "/tmp", focus: false, session: "n2").json() == #"{"cwd":"\/tmp","op":"space_create","req":5,"session":"n2"}"#)
        #expect(try ClientMsg.view(req: nil, window: "phone", space: nil, pane: nil, session: "n2").json() == #"{"op":"view","session":"n2","window":"phone"}"#)
    }

    @Test func showingASpaceShowsItsSession() {
        var view = SessionView()
        view.show(space: "s4", in: two)
        #expect(view.session == "n2" && view.space == "s4")
        view.show(space: "s9", in: two)
        #expect(view.space == "s4", "a space no session holds is ignored")
    }

    @Test func switchingSessionReturnsToTheSpaceLastShownThere() {
        var view = SessionView()
        view.show(space: "s2", in: two)
        view.show(session: "n2", in: two)
        #expect(view.space == "s4")
        view.show(session: "n1", in: two)
        #expect(view.space == "s2")
        let empty = [SessionInfo(id: "n3", name: "new", spaces: [])]
        view.show(session: "n3", in: empty)
        #expect(view.session == "n3" && view.space == nil)
    }

    @Test func aMovedSpaceTakesTheViewWithIt() {
        var view = SessionView()
        view.show(space: "s2", in: two)
        let moved = [SessionInfo(id: "n1", name: "wheel", spaces: ["s1", "s3"]), SessionInfo(id: "n2", name: "games", spaces: ["s4", "s2"])]
        view.layoutChanged(from: two, to: moved)
        #expect(view.session == "n2" && view.space == "s2")
    }

    @Test func aClosedSpaceGivesWayToItsNeighbourInTheSameSession() {
        var view = SessionView()
        view.show(space: "s2", in: two)
        let closed = [SessionInfo(id: "n1", name: "wheel", spaces: ["s1", "s3"]), two[1]]
        view.layoutChanged(from: two, to: closed)
        #expect(view.session == "n1" && view.space == "s3", "the next space")
        let last = [SessionInfo(id: "n1", name: "wheel", spaces: ["s1"]), two[1]]
        view.layoutChanged(from: closed, to: last)
        #expect(view.space == "s1", "the one before when it was last")
        let empty = [SessionInfo(id: "n1", name: "wheel", spaces: []), two[1]]
        view.layoutChanged(from: last, to: empty)
        #expect(view.session == "n1" && view.space == nil, "an emptied session stays shown, never another session's space")
    }

    @Test func aClosedSessionGivesWayToTheFirst() {
        var view = SessionView()
        view.show(space: "s4", in: two)
        view.layoutChanged(from: two, to: [two[0]])
        #expect(view.session == "n1" && view.space == "s1")
        view.layoutChanged(from: [two[0]], to: [])
        #expect(view.session == nil && view.space == nil)
    }

    @Test func restoresTheSavedViewOrTheNearestThatIsLeft() {
        let space = SessionView.restored(session: "n1", space: "s3", in: two)
        #expect(space.view.space == "s3" && space.view.session == "n1" && space.took == "space")
        let session = SessionView.restored(session: "n2", space: "gone", in: two)
        #expect(session.view.space == "s4" && session.took == "session")
        let first = SessionView.restored(session: "gone", space: "gone", in: two)
        #expect(first.view.space == "s1" && first.took == "first session")
    }

    private func space(_ id: String, name: String = "wheel", statuses: [String: PaneStatus], status: PaneStatus? = nil) -> SpaceInfo {
        SpaceInfo(id: id, name: name, cwd: "/tmp", root: .split(id: "x1", axis: .horizontal, ratio: 0.5, a: .pane(id: "p1", kind: .empty), b: .pane(id: "p2", kind: .empty)),
                  focus: "p1", status: status, paneStatuses: statuses)
    }

    @Test func captionsCountPanesAndTheirStatuses() {
        let all = Set(StatusRank.allCases)
        #expect(SessionCaption.space(space("s1", statuses: ["p1": .working, "p2": .working]), shown: all) == "2 panes · 2 working")
        #expect(SessionCaption.space(space("s1", statuses: ["p1": .needsYou(waiting: 1), "p2": .idle]), shown: all) == "2 panes · 1 needs you")
        #expect(SessionCaption.space(space("s1", statuses: ["p1": .working]), shown: [.needsYou]) == "2 panes", "a rank status.dots hides says nothing")
        let spaces = [space("s1", statuses: [:], status: .needsYou(waiting: 1)), space("s2", statuses: [:], status: .working), space("s3", statuses: [:], status: .working)]
        #expect(SessionCaption.session(two[0], spaces: spaces, shown: all) == "3 spaces · 1 needs you · 2 working")
        #expect(SessionCaption.session(two[1], spaces: spaces, shown: all) == "1 space · idle")
        #expect(SessionCaption.rank(of: two[0], spaces: spaces) == .needsYou)
    }

    @Test func closingAsksOnlyWhileAnAgentWorks() {
        #expect(SessionCaption.closeQuestion(space("s1", statuses: ["p1": .working, "p2": .working])) == "Close wheel? 2 agents are working. Closing ends them and every terminal in it.")
        #expect(SessionCaption.closeQuestion(space("s1", statuses: ["p1": .working, "p2": .idle])) == "Close wheel? 1 agent is working. Closing ends it and every terminal in it.")
        #expect(SessionCaption.closeQuestion(space("s1", statuses: ["p1": .needsYou(waiting: 1)])) == nil)
    }
}
