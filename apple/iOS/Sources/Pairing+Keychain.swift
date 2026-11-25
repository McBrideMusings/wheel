import Foundation
import Security
import WheelKit

/// Every wheeld this phone is paired with, in the order they were paired, kept in the Keychain as
/// one JSON list of the pairings their codes carried with each device credential (ADR-0057).
enum PairingStore {
    private static func query(_ account: String) -> [String: Any] { [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "com.piercemakes.wheel.phone",
        kSecAttrAccount as String: account,
    ] }

    private static let listAccount = "pairings"
    /// Where a build that held one pairing kept it; `all()` moves it into the list once.
    private static let singleAccount = "pairing"

    static func all() -> [Pairing] {
        if let list: [Pairing] = read(listAccount) { return list }
        guard let single: Pairing = read(singleAccount) else { return [] }
        let list = [single]
        if write(list) == errSecSuccess { SecItemDelete(query(singleAccount) as CFDictionary) }
        return list
    }

    /// Keep `pairing`, in place of the entry for the same wheeld when there is one.
    @discardableResult
    static func keep(_ pairing: Pairing) -> OSStatus {
        write(all().keeping(pairing))
    }

    /// Forget the wheeld `pairing` reaches.
    @discardableResult
    static func remove(_ pairing: Pairing) -> OSStatus {
        write(all().filter { $0.url != pairing.url })
    }

    private static func read<T: Decodable>(_ account: String) -> T? {
        var q = query(account)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    private static func write(_ list: [Pairing]) -> OSStatus {
        guard let data = try? JSONEncoder().encode(list) else { return errSecParam }
        SecItemDelete(query(listAccount) as CFDictionary)
        var q = query(listAccount)
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(q as CFDictionary, nil)
    }
}
