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
            "action:open-folder", "action:settings", "action:toggle-sidebar", "action:chat-terminal",
        ])
    }

    @Test func letterScatteredThroughAPathMatchesAndWordStartsAndTheLastComponentScoreHigher() throws {
        let plain = try #require(CommandPalette.score("tmpa", in: "/tmp/a"))
        #expect(CommandPalette.score("tmpb", in: "/tmp/a") == nil)
        // Letters in order but scattered score below the same letters side by side.
        let scattered = try #require(CommandPalette.score("wh", in: "/Users/ada/work/hold"))
        let together = try #require(CommandPalette.score("wh", in: "/Users/ada/wheel"))
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
        #expect(p.selectedItem == .openFolder)
        p.type("zzzzz")
        #expect(p.rows.isEmpty && p.selectedItem == nil)
    }
}
