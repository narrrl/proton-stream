import Foundation
import Security

/// The share URL fragment, custom passwords and metadata API keys, held in the
/// iOS Keychain. The counterpart of Android's `KeystoreSecretStore`.
///
/// Anything thrown from here reaches Rust as `UnexpectedUniFFICallbackError`,
/// which the bridge turns into an ordinary `BridgeError` rather than a panic on
/// whichever tokio worker asked (B18).
final class KeychainSecretStore: AndroidSecretStore, @unchecked Sendable {
    private let service = "io.narl.protonstream.secrets"

    struct KeychainError: Error, CustomStringConvertible {
        let status: OSStatus
        let action: String
        var description: String {
            let message = SecCopyErrorMessageString(status, nil) as String? ?? "status \(status)"
            return "keychain \(action): \(message)"
        }
    }

    private func query(_ key: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
    }

    func set(key: String, value: String) throws {
        let data = Data(value.utf8)
        let update = SecItemUpdate(query(key) as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if update == errSecSuccess { return }
        guard update == errSecItemNotFound else { throw KeychainError(status: update, action: "update") }
        var item = query(key)
        item[kSecValueData as String] = data
        // Readable by the refresh that runs while the phone is locked in a
        // pocket, and never restored onto another device from a backup.
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let add = SecItemAdd(item as CFDictionary, nil)
        guard add == errSecSuccess else { throw KeychainError(status: add, action: "add") }
    }

    func get(key: String) throws -> String? {
        var item = query(key)
        item[kSecReturnData as String] = true
        item[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(item as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else {
            throw KeychainError(status: status, action: "read")
        }
        return String(data: data, encoding: .utf8)
    }

    func delete(key: String) throws {
        let status = SecItemDelete(query(key) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainError(status: status, action: "delete")
        }
    }
}
