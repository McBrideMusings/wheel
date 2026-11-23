import Testing
@testable import WheelKit

struct OrchestratorSpaceTests {
    @Test func theOrchestratorFolderIsTheSettingElseBesideTheMachineFile() {
        let setting = { (value: String) in
            SettingInfo(key: "orchestrator.dir", value: value, origin: .default, default: "", help: "", machineOnly: true,
                        machineFile: "/Users/p/.wheel/wheel.toml")
        }
        #expect(setting("").orchestratorDir == "/Users/p/.wheel/orchestrator")
        #expect(setting(" /o/dir ").orchestratorDir == "/o/dir")
    }
}
