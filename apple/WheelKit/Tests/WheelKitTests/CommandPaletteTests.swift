import Foundation
import Testing
@testable import WheelKit

struct CommandPaletteTests {
    static let actions = [
        PaletteAction(id: "open-folder", title: "Open Folder…"),
        PaletteAction(id: "settings", title: "Settings", shortcut: "⌘,"),
        PaletteAction(id: "toggle-sidebar", title: "Toggle Sidebar"),
        PaletteAction(id: "chat-terminal", title: "Switch Agent Pane Between Chat and Terminal"),
    ]

    private func palette(spaces: [CommandPalette.OpenSpace] = [], recents: [RecentDir] = []) -> CommandPalette {
        var p = CommandPalette()
        p.list(spaces: spaces, recents: recents, actions: Self.actions)
        return p
    }

    @Test func anEmptyQueryListsOpenSpacesThenClosedRecentsNewestFirstThenActions() {
        let p = palette(
            spaces: [.init(id: "s2", name: "wheel", dir: "/Users/p/wheel"), .init(id: "s1", name: "tmp", dir: "/tmp")],
            recents: [
                RecentDir(dir: "/Users/p/old", openedMs: 10),
                RecentDir(dir: "/tmp", openedMs: 50),
                RecentDir(dir: "/Users/p/new", openedMs: 30),
            ]
        )
        #expect(p.rows.map(\.id) == [
            "space:s2", "space:s1", "recent:/Users/p/new", "recent:/Users/p/old",
            "action:new-space", "action:open-folder", "action:settings", "action:toggle-sidebar", "action:chat-terminal",
        ])
    }

    @Test func panesAndArtifactsListAfterSpacesAndAQueryFindsThemByName() {
        var p = CommandPalette()
        p.list(
            spaces: [.init(id: "s1", name: "wheel", dir: "/w")],
            panes: [.init(id: "p1", space: "s1", spaceName: "wheel", name: "Terminal"),
                    .init(id: "p2", space: "s1", spaceName: "wheel", name: "dash")],
            artifacts: [ArtifactInfo(scope: .project, name: "status", title: "Build Status")],
            recents: [], actions: Self.actions
        )
        #expect(p.rows.prefix(4).map(\.id) == ["space:s1", "pane:p1", "pane:p2", "artifact:project/status"])
        p.type("BUILD")
        #expect(p.rows.map(\.id) == ["artifact:project/status"])
        p.type("term")
        #expect(p.rows.first?.item == .pane(id: "p1", space: "s1", name: "Terminal", spaceName: "wheel"))
    }

    @Test func letterScatteredThroughAPathMatchesAndWordStartsAndTheLastComponentScoreHigher() throws {
        let plain = try #require(CommandPalette.score("tmpa", in: "/tmp/a"))
        #expect(CommandPalette.score("tmpb", in: "/tmp/a") == nil)
        // Letters in order but scattered score below the same letters side by side.
        let scattered = try #require(CommandPalette.score("wh", in: "/Users/pierce/work/hold"))
        let together = try #require(CommandPalette.score("wh", in: "/Users/pierce/wheel"))
        #expect(together > scattered)
        // A match in the path's last component beats the same letters in a parent directory.
        let parent = try #require(CommandPalette.score("wheel", in: "/Users/p/wheel/docs"))
        let leaf = try #require(CommandPalette.score("wheel", in: "/Users/p/docs/wheel"))
        #expect(leaf > parent)
        // A word start beats a letter buried mid-word.
        let start = try #require(CommandPalette.score("sb", in: "Toggle Sidebar"))
        let buried = try #require(CommandPalette.score("sb", in: "Toggle Absorb"))
        #expect(start > buried)
        #expect(plain > 0)
    }

    @Test func aQueryRanksTheBestMatchFirstAndMovingStopsAtTheEnds() {
        var p = palette(recents: [RecentDir(dir: "/tmp/b", openedMs: 2), RecentDir(dir: "/tmp/a", openedMs: 1)])
        p.type("tmpa")
        #expect(p.rows.map(\.id) == ["recent:/tmp/a"])
        #expect(p.selectedItem == .recent(dir: "/tmp/a"))
        p.type("sett")
        #expect(p.rows.first?.item == .action(id: "settings", title: "Settings", shortcut: "⌘,"))
        p.type("")
        p.move(-1)
        #expect(p.selected == 0)
        p.move(100)
        #expect(p.selected == p.rows.count - 1)
        p.type("folder")
        #expect(p.selectedItem == .openFolder())
        p.type("zzzzz")
        #expect(p.rows.isEmpty && p.selectedItem == nil)
    }

    private func newSpace(query: String = "", spaces: [CommandPalette.OpenSpace] = [], recents: [RecentDir] = []) -> CommandPalette {
        var p = CommandPalette(mode: .newSpace)
        p.list(spaces: spaces, recents: recents, actions: Self.actions)
        p.setRoots(["/Users/p/Projects"])
        p.listed("/Users/p/Projects", .init(dirs: [
            DirEntry(name: "wheel", path: "/Users/p/Projects/wheel"),
            DirEntry(name: "wharf", path: "/Users/p/Projects/wharf", space: "s9"),
            DirEntry(name: "notes", path: "/Users/p/Projects/notes"),
        ], error: nil))
        p.listed("~/Projects", .init(dirs: [
            DirEntry(name: "wheel", path: "/Users/p/Projects/wheel"),
            DirEntry(name: "notes", path: "/Users/p/Projects/notes"),
        ], error: nil))
        p.type(query)
        return p
    }

    @Test func aPathQuerySplitsAtItsLastSlashAndAnythingElseIsNotAPath() {
        #expect(CommandPalette.splitPath("~/Projects/wh")?.parent == "~/Projects/")
        #expect(CommandPalette.splitPath("~/Projects/wh")?.needle == "wh")
        #expect(CommandPalette.splitPath("/tmp/")?.parent == "/tmp/")
        #expect(CommandPalette.splitPath("/tmp/")?.needle == "")
        #expect(CommandPalette.splitPath("/x")?.parent == "/")
        #expect(CommandPalette.splitPath("~")?.parent == "~/")
        #expect(CommandPalette.splitPath("whe") == nil)
        #expect(CommandPalette.listingKey("~/Projects/") == "~/Projects")
        #expect(CommandPalette.listingKey("/") == "/")
    }

    @Test func aNameQueryScoresTheRecentsAndEveryRootsFoldersAndFinderComesLast() {
        let p = newSpace(query: "whe", recents: [RecentDir(dir: "/tmp/other", openedMs: 1)])
        #expect(p.rows.map(\.id) == ["folder:/Users/p/Projects/wheel", "action:open-folder"])
        #expect(p.rows.last?.item.title == "Choose in Finder…")
        let all = newSpace(recents: [RecentDir(dir: "/tmp/other", openedMs: 1)])
        #expect(all.rows.map(\.id) == [
            "folder:/tmp/other", "folder:/Users/p/Projects/wheel", "folder:/Users/p/Projects/wharf",
            "folder:/Users/p/Projects/notes", "action:open-folder",
        ])
    }

    @Test func aFolderThatIsOpenSaysSoAndNamesItsSpace() {
        let p = newSpace(query: "wha", spaces: [.init(id: "s2", name: "tmp", dir: "/Users/p/Projects/wheel")])
        #expect(p.rows.first?.item == .folder(path: "/Users/p/Projects/wharf", space: "s9"))
        #expect(p.rows.first?.item.detail == "open · /Users/p/Projects/wharf")
        let byRecent = newSpace(query: "whee", spaces: [.init(id: "s2", name: "tmp", dir: "/Users/p/Projects/wheel")])
        #expect(byRecent.rows.first?.item == .folder(path: "/Users/p/Projects/wheel", space: "s2"))
    }

    @Test func tabCompletesTheSelectedFolderKeepingHowThePathWasTyped() {
        var p = newSpace(query: "~/Projects/wh")
        #expect(p.browsedParent == "~/Projects")
        #expect(p.selectedItem == .folder(path: "/Users/p/Projects/wheel", space: nil))
        p.tab()
        #expect(p.query == "~/Projects/wheel/")
        #expect(p.browsedParent == "~/Projects/wheel")
        var named = newSpace(query: "whe")
        #expect(named.browsedParent == nil)
        named.tab()
        #expect(named.query == "/Users/p/Projects/wheel/")
        var home = newSpace(query: "~")
        home.listed("~", .init(dirs: [DirEntry(name: "Documents", path: "/Users/p/Documents")], error: nil))
        home.tab()
        #expect(home.query == "~/Documents/")
    }

    @Test func aPathTheDaemonCouldNotListGivesANoFolderRowAboveChooseInFinder() {
        var p = newSpace(query: "~/Projcts/wh")
        p.listed("~/Projcts", .init(dirs: [], error: "no folder at /Users/p/Projcts"))
        #expect(p.rows.map(\.id) == ["missing:~/Projcts/", "action:open-folder"])
        #expect(p.rows.first?.item.title == "No folder at ~/Projcts/")
        p.tab()
        #expect(p.query == "~/Projcts/wh")
    }
}
