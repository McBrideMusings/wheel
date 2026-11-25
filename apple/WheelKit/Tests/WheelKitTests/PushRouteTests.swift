import XCTest
@testable import WheelKit

final class PushRouteTests: XCTestCase {
    /// The same pane id on two servers: each push opens the server that sent it.
    func testAPushOpensTheServerThatSentIt() {
        XCTAssertEqual(PushRoute.route(server: "dev", among: ["mac", "dev"]), .open(1))
        XCTAssertEqual(PushRoute.route(server: "mac", among: ["mac", "dev"]), .open(0))
    }

    func testAPushFromAnUnpairedServerOpensNothing() {
        XCTAssertEqual(PushRoute.route(server: "other", among: ["mac", "dev"]), .none)
        XCTAssertEqual(PushRoute.route(server: "other", among: []), .none)
    }

    func testAServerWithNoIdYetMakesTheTapWait() {
        XCTAssertEqual(PushRoute.route(server: "dev", among: ["mac", nil]), .wait)
        XCTAssertEqual(PushRoute.route(server: "mac", among: ["mac", nil]), .open(0))
    }
}
