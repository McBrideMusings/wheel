import Foundation
import Testing
@testable import WheelUI

@MainActor
struct A2UISurfaceTests {
    private func surface() throws -> SurfaceModel {
        let json = #"""
        [{"createSurface":{"surfaceId":"s"}},
         {"updateComponents":{"components":[
           {"id":"root","component":"Column","children":["q","go"]},
           {"id":"q","component":"TextField","label":"Search","value":{"path":"/q"}},
           {"id":"go","component":"Button","child":"t","action":{"event":{"name":"run","context":{"q":{"path":"/q"}}}}},
           {"id":"t","component":"Text","text":"Run"}]}},
         {"updateDataModel":{"path":"/q","value":"bug"}}]
        """#
        let messages = try JSONDecoder().decode([[String: AnyCodable]].self, from: Data(json.utf8))
        let model = SurfaceModel()
        #expect(model.process(messages).isEmpty)
        return model
    }

    @Test func aBoundPropertyReadsTheDataModelAtItsPath() throws {
        let m = try surface()
        #expect(m.node("q")?.instance.properties["value"] == .string("bug"))
        #expect(m.root?.children.map(\.instance.id) == ["q", "go"])
    }

    @Test func aControlWritingAValueShowsAtOnceAndRaisesOneChange() throws {
        let m = try surface()
        var changes: [A2UIChange] = []
        m.onChange = { changes.append($0) }
        m.set("/q", .string("bugs"))
        m.set("/q", .string("bugs"))
        #expect(m.get("/q") == .string("bugs"))
        #expect(changes == [A2UIChange(path: "/q", value: .string("bugs"))])
    }

    @Test func aServerWriteRaisesNoChange() throws {
        let m = try surface()
        var changes = 0
        m.onChange = { _ in changes += 1 }
        m.process([["updateDataModel": .dictionary(["path": .string("/q"), "value": .string("x")])]])
        #expect(m.get("/q") == .string("x"))
        #expect(changes == 0)
    }

    @Test func aControlWithNoBindingWritesUnderItsId() throws {
        let m = SurfaceModel()
        m.process([["updateComponents": .dictionary(["components": .array([.dictionary(["id": .string("c"), "component": .string("CheckBox"), "value": .bool(false)])])])]])
        #expect(a2uiPath(of: m.components["c"]!, in: m) == "/c")
    }

    @Test func aServerWriteToAHeldPathWaitsUntilItIsReleased() throws {
        let m = try surface()
        m.held.insert("/q")
        m.process([["updateDataModel": .dictionary(["path": .string("/q"), "value": .string("stale")])]])
        #expect(m.get("/q") == .string("bug"))
        m.held.remove("/q")
        m.process([["updateDataModel": .dictionary(["path": .string("/q"), "value": .string("fresh")])]])
        #expect(m.get("/q") == .string("fresh"))
    }

    @Test func deletingTheSurfaceResetsTheChangeToolAndHeldPaths() throws {
        let m = SurfaceModel()
        m.process([["createSurface": .dictionary(["changeTool": .string("edited")])]])
        m.held.insert("/q")
        m.process([["deleteSurface": .dictionary([:])]])
        #expect(m.changeTool == "data_changed")
        #expect(m.held.isEmpty)
    }
}
