import CryptoKit
import ImageIO
import SwiftUI
import UIKit

/// One file to pull Proton's own thumbnail from when no provider poster exists.
///
/// Proton renders a thumbnail per file, so any episode of a show is a frame of
/// the right show — the desktop client picks the first for the same reason.
struct ThumbnailSource: Hashable, Sendable {
    let shareId: String
    let volumeId: String
    let linkId: String

    var key: String { "\(shareId)/\(linkId)" }
}

extension TitleRecord {
    /// What the first episode of a title offers as a poster. Metadata lookups
    /// are off by default, so for a freshly crawled library this is the only
    /// artwork there is.
    var thumbnailSource: ThumbnailSource? {
        seasons.lazy.compactMap(\.episodes.first).first.map(\.thumbnailSource)
    }
}

extension EpisodeRecord {
    var thumbnailSource: ThumbnailSource { ThumbnailSource(shareId: shareId, volumeId: volumeId, linkId: linkId) }
}

/// HTTPS-only artwork, cropped to whatever frame its parent gives it.
///
/// `fallback` is used only when there is no provider artwork or it fails to
/// load, which — with metadata off — is the common case rather than the rare
/// one. Until either answers, the box pulses as a skeleton; when neither has
/// anything, it shows `ArtworkPlaceholder` rather than an empty card — without
/// initials when `labelled` is false, for art with the name already over it.
struct RemoteArtwork: View {
    let url: String?
    let description: String
    var fallback: ThumbnailSource?
    var labelled = true
    @State private var load: ArtworkLoad

    init(_ url: String?, _ description: String, fallback: ThumbnailSource? = nil, labelled: Bool = true) {
        self.url = url
        self.description = description
        self.fallback = fallback
        self.labelled = labelled
        // A poster already decoded draws in the first frame, so scrolling back
        // over the grid does not flash a skeleton per tile.
        _load = State(initialValue: ArtworkLoader.shared.cached(url: url, fallback: fallback).map(ArtworkLoad.loaded) ?? .pending)
    }

    var body: some View {
        Color.clear
            .overlay {
                switch load {
                case .pending:
                    Skeleton()
                case .missing:
                    ArtworkPlaceholder(name: description, labelled: labelled)
                case let .loaded(image):
                    Image(uiImage: image).resizable().scaledToFill()
                }
            }
            .clipped()
            .accessibilityElement()
            .accessibilityLabel(description)
            .task(id: Request(url: url, fallback: fallback)) {
                if case .loaded = load, ArtworkLoader.shared.cached(url: url, fallback: fallback) != nil { return }
                let loaded = await ArtworkLoader.shared.artwork(url: url, fallback: fallback)
                if Task.isCancelled { return }
                load = loaded.map(ArtworkLoad.loaded) ?? .missing
            }
    }

    private struct Request: Hashable {
        let url: String?
        let fallback: ThumbnailSource?
    }
}

private enum ArtworkLoad {
    case pending
    case missing
    case loaded(UIImage)
}

/// Something being loaded: the surface tint, breathing.
///
/// A spinner per tile is a grid of spinners; a shape where the poster will be
/// says what is coming and where, and settles the layout before it arrives.
struct Skeleton: View {
    @Environment(\.scheme) private var scheme
    @State private var dim = false

    var body: some View {
        scheme.surfaceVariant
            .opacity(dim ? 0.55 : 1)
            .onAppear {
                withAnimation(.linear(duration: 0.8).repeatForever(autoreverses: true)) { dim = true }
            }
    }
}

/// Art for a title that has none: the accent as a gradient, and its initials.
///
/// Each name sets where on the accent ramp its gradient starts, so neighbours
/// differ while every one stays in the theme's colours — and since the hash is
/// Java's, a title gets the same colour here as on Android.
struct ArtworkPlaceholder: View {
    let name: String
    var labelled = true
    @Environment(\.scheme) private var scheme

    var body: some View {
        let start = mix(scheme.primary, scheme.secondary, Double(javaHash(name).modulo(5)) / 4)
        GeometryReader { geometry in
            LinearGradient(colors: [start, mix(start, .black, 0.55)], startPoint: .topLeading, endPoint: .bottomTrailing)
                .overlay {
                    if labelled {
                        Text(initials(name))
                            .font(.inter(size: min(geometry.size.width, geometry.size.height) * 0.3, semibold: true))
                            .foregroundStyle(Color.white.opacity(0.9))
                            .lineLimit(1)
                    }
                }
        }
    }
}

/// "Oshi no Ko" is "OK", "Akira" is "A": the first letters of up to two
/// capitalised words, so particles and articles in a romanised name do not
/// take a slot. A name with no capitals uses its first two words.
func initials(_ name: String) -> String {
    let words = name
        .split(whereSeparator: { " -_.:".contains($0) })
        .filter { $0.first.map { $0.isLetter || $0.isNumber } ?? false }
    let capitalised = words.filter { $0.first!.isUppercase || $0.first!.isNumber }
    return (capitalised.isEmpty ? words : capitalised).prefix(2).map { $0.first!.uppercased() }.joined()
}

/// `String.hashCode()` from the JVM, over UTF-16 code units.
func javaHash(_ text: String) -> Int32 {
    text.utf16.reduce(Int32(0)) { hash, unit in hash &* 31 &+ Int32(unit) }
}

private extension Int32 {
    /// Kotlin's `mod`: never negative.
    func modulo(_ divisor: Int32) -> Int32 {
        let remainder = self % divisor
        return remainder < 0 ? remainder + divisor : remainder
    }
}

/// The HTTPS-only fetch behind every poster, its disk cache under
/// `Caches/metadata-art`, and Proton's own thumbnails as the fallback.
final class ArtworkLoader: @unchecked Sendable {
    static let shared = ArtworkLoader()

    /// Decoded posters, so scrolling the grid re-reads neither disk nor
    /// decoder. Keyed by artwork URL or by `share/link` for a Proton
    /// thumbnail; the two cannot collide, since one is always an `https:` URL.
    private let decoded: NSCache<NSString, UIImage> = {
        let cache = NSCache<NSString, UIImage>()
        cache.totalCostLimit = 32 * 1024 * 1024
        return cache
    }()

    /// Files known to have no Proton thumbnail, so the answer is paid for once.
    /// Deliberately not persisted: a re-upload can add one.
    private var missing = Set<String>()
    private let lock = NSLock()

    private let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 10
        configuration.urlCache = nil
        return URLSession(configuration: configuration)
    }()

    private static let maxBytes = 12 * 1024 * 1024
    private static let maxCacheBytes = 48 * 1024 * 1024
    /// Longest edge worth decoding: a poster on a phone, or lock-screen art.
    private static let maxEdge = 1024

    func cached(url: String?, fallback: ThumbnailSource?) -> UIImage? {
        if let url, let image = decoded.object(forKey: url as NSString) { return image }
        if let key = fallback?.key { return decoded.object(forKey: key as NSString) }
        return nil
    }

    func artwork(url: String?, fallback: ThumbnailSource?) async -> UIImage? {
        if let image = await remote(url) { return image }
        return await thumbnail(fallback)
    }

    /// Provider artwork, from the disk cache or the network.
    func remote(_ requested: String?) async -> UIImage? {
        guard let requested else { return nil }
        if let image = decoded.object(forKey: requested as NSString) { return image }
        guard let url = URL(string: requested), url.scheme?.lowercased() == "https" else { return nil }
        let directory = Self.directory
        let digest = SHA256.hash(data: Data(requested.utf8)).map { String(format: "%02x", $0) }.joined()
        let cachedFile = directory.appendingPathComponent("\(digest).img")
        let files = FileManager.default
        let fresh = !files.fileExists(atPath: cachedFile.path)
        let bytes: Data
        if fresh {
            guard let downloaded = await download(url) else { return nil }
            bytes = downloaded
        } else {
            guard let read = try? Data(contentsOf: cachedFile) else { return nil }
            bytes = read
        }
        guard let image = Self.decodeSampled(bytes) else { return nil }
        if fresh {
            // Written only once it has decoded: caching the bytes first is what
            // turns one hostile poster into a failure on every launch.
            let temporary = directory.appendingPathComponent("\(digest).part")
            if (try? bytes.write(to: temporary)) != nil {
                try? files.moveItem(at: temporary, to: cachedFile)
                Self.prune(directory)
            }
        } else {
            try? files.setAttributes([.modificationDate: Date()], ofItemAtPath: cachedFile.path)
        }
        decoded.setObject(image, forKey: requested as NSString, cost: image.cost)
        return image
    }

    /// Proton's own thumbnail for one file, decoded. A failed call is not
    /// remembered — an unreachable share is transient — but a file with no
    /// thumbnail is, for the session.
    func thumbnail(_ source: ThumbnailSource?) async -> UIImage? {
        guard let source else { return nil }
        let key = source.key
        if let image = decoded.object(forKey: key as NSString) { return image }
        if lock.withLock({ missing.contains(key) }) { return nil }
        let fetched: Data?
        do {
            fetched = try await NativeRuntime.engine().thumbnail(shareId: source.shareId, volumeId: source.volumeId, linkId: source.linkId)
        } catch {
            return nil
        }
        guard let bytes = fetched, let image = Self.decodeSampled(bytes) else {
            lock.withLock { _ = missing.insert(key) }
            return nil
        }
        decoded.setObject(image, forKey: key as NSString, cost: image.cost)
        return image
    }

    private func download(_ url: URL) async -> Data? {
        guard let (stream, response) = try? await session.bytes(from: url),
              let http = response as? HTTPURLResponse,
              (200 ... 299).contains(http.statusCode),
              // A redirect must not leave HTTPS.
              http.url?.scheme?.lowercased() == "https"
        else { return nil }
        var bytes = Data()
        do {
            for try await byte in stream {
                bytes.append(byte)
                if bytes.count > Self.maxBytes { return nil }
            }
        } catch {
            return nil
        }
        return bytes
    }

    /// Decode within a bound the display can actually use: the compressed size
    /// says nothing about what an image costs decoded.
    private static func decodeSampled(_ bytes: Data) -> UIImage? {
        guard let source = CGImageSourceCreateWithData(bytes as CFData, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: maxEdge,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        return UIImage(cgImage: image)
    }

    private static var directory: URL {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let directory = caches.appendingPathComponent("metadata-art", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    /// Hold the cache directory under `maxCacheBytes`, oldest first.
    private static func prune(_ directory: URL) {
        let keys: [URLResourceKey] = [.contentModificationDateKey, .fileSizeKey]
        guard let files = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: keys) else { return }
        let entries = files.compactMap { file -> (URL, Date, Int)? in
            guard let values = try? file.resourceValues(forKeys: Set(keys)) else { return nil }
            return (file, values.contentModificationDate ?? .distantPast, values.fileSize ?? 0)
        }.sorted { $0.1 < $1.1 }
        var total = entries.reduce(0) { $0 + $1.2 }
        for (file, _, size) in entries {
            if total <= maxCacheBytes { return }
            total -= size
            try? FileManager.default.removeItem(at: file)
        }
    }
}

private extension UIImage {
    var cost: Int {
        guard let image = cgImage else { return 0 }
        return image.bytesPerRow * image.height
    }
}
