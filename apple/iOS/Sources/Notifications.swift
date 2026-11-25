import UIKit
import UserNotifications
import WheelKit

/// The phone's side of push notifications: ask to show them, hand Apple's device token to the model
/// for the daemon, and open the pane a tapped notification names.
///
/// The daemon sends the notification itself (`push.apns.*` settings); the payload carries `server`
/// (the wheeld's id), `space` and `pane` beside `aps`. Apple only issues a token to an app signed with the Push Notifications
/// capability, so without it `registerForRemoteNotifications` fails and the failure is logged.
@MainActor
final class PushCenter: NSObject, UNUserNotificationCenterDelegate {
    static let shared = PushCenter()

    /// Called with the hex device token each time Apple gives one.
    var onToken: ((String) -> Void)?
    /// Called with the server, space and pane a tapped notification names.
    var onOpen: ((PushTarget) -> Void)?
    private var token: String?
    private var pendingOpen: PushTarget?

    /// Ask to show notifications, and for a token once allowed. Safe to call on every launch.
    func start() {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, error in
            Task { @MainActor in
                Log.info("notifications \(granted ? "allowed" : "not allowed")\(error.map { ": \($0.localizedDescription)" } ?? "")")
                if granted { UIApplication.shared.registerForRemoteNotifications() }
            }
        }
    }

    func received(deviceToken data: Data) {
        let hex = data.map { String(format: "%02x", $0) }.joined()
        token = hex
        Log.info("push token received (\(hex.count / 2) bytes)")
        onToken?(hex)
    }

    func failed(_ error: Error) {
        Log.info("push token unavailable: \(error.localizedDescription)")
    }

    /// Hands a tap that arrived before the model was listening to it once it is.
    func listen(onOpen: @escaping (PushTarget) -> Void) {
        self.onOpen = onOpen
        if let target = pendingOpen {
            pendingOpen = nil
            onOpen(target)
        }
    }

    // The app on screen shows needs-you itself; a banner over it would say the same thing twice.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        []
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let info = response.notification.request.content.userInfo
        guard let server = info["server"] as? String, let space = info["space"] as? String,
              let pane = info["pane"] as? String else { return }
        let target = PushTarget(server: server, space: space, pane: pane)
        await MainActor.run {
            Log.info("notification tapped: pane \(pane) in \(space) on server \(server)")
            if let onOpen { onOpen(target) } else { pendingOpen = target }
        }
    }
}

/// The pane a tapped notification opens: ids are per server, so the server comes with them.
struct PushTarget {
    let server: String
    let space: String
    let pane: String
}

final class PhoneAppDelegate: NSObject, UIApplicationDelegate {
    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Task { @MainActor in PushCenter.shared.received(deviceToken: deviceToken) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Task { @MainActor in PushCenter.shared.failed(error) }
    }
}
