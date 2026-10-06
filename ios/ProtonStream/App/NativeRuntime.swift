import Foundation

/// Process-wide owner of the Rust engine; records and decrypted media remain
/// app-private. The counterpart of Android's `NativeRuntime`.
enum NativeRuntime {
    private static let lock = NSLock()
    private static var instance: AndroidEngine?

    /// The engine, built on first use off the main thread: construction opens
    /// SQLite, spins a Tokio runtime and reads the Keychain.
    static func engine() async throws -> AndroidEngine {
        try await Task.detached(priority: .userInitiated) { try blockingEngine() }.value
    }

    /// The engine from a thread that may block — a bridge callback, a
    /// background task. Never call this on the main thread.
    static func blockingEngine() throws -> AndroidEngine {
        try lock.withLock {
            if let instance { return instance }
            let engine = try AndroidEngine(paths: try paths(), secrets: KeychainSecretStore())
            // The budget is this app's setting, so the engine starts on the
            // default and is told the chosen one before anything streams.
            engine.setStreamCacheBudget(bytes: SettingsStore.gibToBytes(SettingsStore().cacheBudgetGib))
            instance = engine
            return engine
        }
    }

    /// The stored palette, without building the engine.
    ///
    /// Callable from the main thread, and meant to be: it is the only palette
    /// read early enough to decide what the first frame is painted in, and
    /// `engine()` is not. This reads one small JSON file and resolves it.
    static func initialPalette() -> PaletteRecord? {
        guard let paths = try? paths() else { return nil }
        return storedPalette(paths: paths)
    }

    /// Application Support for the catalog and the configuration, Caches for
    /// the block cache, which iOS may purge.
    ///
    /// Neither the catalog nor the configuration is backed up, as on Android
    /// (`allowBackup="false"`): the share secrets live in the Keychain as
    /// this-device-only items, so a restored share list would arrive without
    /// them, and a restored `sync.json` would give two phones one sync identity
    /// writing one file — the case `pstr-core::sync` exists to prevent.
    private static func paths() throws -> AndroidPaths {
        let files = FileManager.default
        let support = try files.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        let caches = try files.url(for: .cachesDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        var config = support.appendingPathComponent("config", isDirectory: true)
        var data = support.appendingPathComponent("data", isDirectory: true)
        let cache = caches.appendingPathComponent("stream", isDirectory: true)
        for directory in [config, data, cache] {
            try files.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        try config.setResourceValues(excluded)
        try data.setResourceValues(excluded)
        return AndroidPaths(config: config.path, data: data.path, cache: cache.path)
    }
}

/// What a bridge failure says, without Swift's type name around it.
///
/// `BridgeError`'s own description is `String(reflecting:)`, which reads
/// `PstrBridge.BridgeError.Failure(reason: "…")` in a snackbar.
func errorMessage(_ error: Error) -> String {
    if case let BridgeError.Failure(reason) = error { return reason }
    let text = error.localizedDescription
    return text.isEmpty ? "Unexpected error" : text
}
