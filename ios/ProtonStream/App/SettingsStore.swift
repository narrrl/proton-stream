import Foundation

/// The settings that are this front end's alone.
///
/// Playback preferences — volume, languages, autoplay, auto-skip — deliberately
/// do *not* live here: they are `pstr_core::prefs`, reached over the bridge, so
/// there is one definition of them rather than one per front end. Keys and
/// defaults are Android's `SettingsStore`.
struct SettingsStore {
    private let defaults = UserDefaults.standard

    var wifiOnly: Bool {
        get { bool(Key.wifiOnly, default: true) }
        nonmutating set { defaults.set(newValue, forKey: Key.wifiOnly) }
    }

    var backgroundAudio: Bool {
        get { bool(Key.backgroundAudio, default: true) }
        nonmutating set { defaults.set(newValue, forKey: Key.backgroundAudio) }
    }

    /// Whether mpv may decode in hardware. Off is the way out for a file
    /// VideoToolbox shows green or torn frames for; it costs battery, so it is
    /// on unless the viewer turns it off. Read at each load, so a change applies
    /// from the next file.
    var hardwareDecoding: Bool {
        get { bool(Key.hardwareDecoding, default: true) }
        nonmutating set { defaults.set(newValue, forKey: Key.hardwareDecoding) }
    }

    /// The streaming cache's budget, in GiB. See `cacheBudgetChoices`.
    var cacheBudgetGib: Int {
        get { defaults.object(forKey: Key.cacheBudgetGib) as? Int ?? Self.defaultCacheBudgetGib }
        nonmutating set { defaults.set(newValue, forKey: Key.cacheBudgetGib) }
    }

    /// The desktop's range, in the steps a phone's storage is bought in.
    static let cacheBudgetChoices = [1, 2, 4, 8, 16]

    /// `DiskCacheConfig::DEFAULT_BUDGET_BYTES`, so an unset choice changes nothing.
    static let defaultCacheBudgetGib = 4

    static func gibToBytes(_ gib: Int) -> UInt64 { UInt64(gib) * 1024 * 1024 * 1024 }

    private func bool(_ key: String, default value: Bool) -> Bool {
        defaults.object(forKey: key) as? Bool ?? value
    }

    private enum Key {
        static let wifiOnly = "wifi_only"
        static let backgroundAudio = "background_audio"
        static let hardwareDecoding = "hardware_decoding"
        static let cacheBudgetGib = "cache_budget_gib"
    }
}
