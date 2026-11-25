import XCTest

extension XCUIApplication {
    /// The app paired with `pair` (a `WHEEL_PAIR_JSON` code). It forwards `WHEEL_FORGET_SERVERS`
    /// from the runner, because a pool simulator's Keychain outlives an uninstall and holds earlier pairings.
    static func paired(_ pair: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launchEnvironment["WHEEL_FORGET_SERVERS"] = ProcessInfo.processInfo.environment["WHEEL_FORGET_SERVERS"]
        return app
    }

    /// Relaunches on the pairing the Keychain kept: no code, and nothing forgotten.
    func relaunchFromKeychain() {
        terminate()
        launchEnvironment["WHEEL_PAIR_JSON"] = nil
        launchEnvironment["WHEEL_FORGET_SERVERS"] = nil
        launch()
    }
}
