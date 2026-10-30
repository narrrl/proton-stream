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
/// one. Until either answers, the box shimmers as a skeleton; when neither has
/// anything, it shows `ArtworkPlaceholder` rather than an empty card — without
/// initials when `labelled` is false, for art with the name already over it.
///
/// Art that arrives fades in over the skeleton, as on Android; art already
/// decoded is drawn in the first frame and has nothing to fade.
struct RemoteArtwork: View {
    let url: String?
    let description: String
    var fallback: ThumbnailSource?
    var labelled = true
    /// Told the picture's colour once it has arrived — see `pictureMood`.
    var onMood: ((UInt32) -> Void)?
    @State private var load: ArtworkLoad

    init(
        _ url: String?,
        _ description: String,
        fallback: ThumbnailSource? = nil,
        labelled: Bool = true,
        onMood: ((UInt32) -> Void)? = nil
    ) {
        self.url = url
        self.description = description
        self.fallback = fallback
        self.labelled = labelled
        self.onMood = onMood
        // A poster already decoded draws in the first frame, so scrolling back
        // over the grid does not flash a skeleton per tile.
        _load = State(initialValue: ArtworkLoader.shared.cached(url: url, fallback: fallback).map(ArtworkLoad.loaded) ?? .pending)
    }

    var body: some View {
        Color.clear
            .overlay {
                switch load {
                case .pending:
                    Skeleton().transition(.opacity)
                case .missing:
                    ArtworkPlaceholder(name: description, labelled: labelled).transition(.opacity)
                case let .loaded(image):
                    Image(uiImage: image).resizable().scaledToFill().transition(.opacity)
                }
            }
            .clipped()
            .accessibilityElement()
            .accessibilityLabel(description)
            .task(id: Request(url: url, fallback: fallback)) {
                if case let .loaded(image) = load, ArtworkLoader.shared.cached(url: url, fallback: fallback) != nil {
                    report(image)
                    return
                }
                let loaded = await ArtworkLoader.shared.artwork(url: url, fallback: fallback)
                if Task.isCancelled { return }
                withAnimation(Motion.arrive) { load = loaded.map(ArtworkLoad.loaded) ?? .missing }
                if let loaded { report(loaded) }
            }
    }

    private func report(_ image: UIImage) {
        guard let onMood, let mood = moodOf(image) else { return }
        onMood(mood)
    }

    private struct Request: Hashable {
        let url: String?
        let fallback: ThumbnailSource?
    }
}

/// A picture squeezed to this many pixels a side is still its colours, and a
/// few hundred values to average — Android's `MOOD_EDGE`.
private let moodEdge = 24

/// The colour a picture leaves behind, as `0xAARRGGBB`: the shared
/// `appearance::mood`, over the picture squeezed to `moodEdge` pixels a side.
func moodOf(_ image: UIImage) -> UInt32? {
    guard let cgImage = image.cgImage else { return nil }
    var bytes = [UInt8](repeating: 0, count: moodEdge * moodEdge * 4)
    let drawn: Bool = bytes.withUnsafeMutableBytes { buffer in
        guard let context = CGContext(
            data: buffer.baseAddress,
            width: moodEdge,
            height: moodEdge,
            bitsPerComponent: 8,
            bytesPerRow: moodEdge * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return false }
        context.interpolationQuality = .medium
        context.draw(cgImage, in: CGRect(x: 0, y: 0, width: moodEdge, height: moodEdge))
        return true
    }
    guard drawn else { return nil }
    let pixels: [UInt32] = stride(from: 0, to: bytes.count, by: 4).map { index in
        let alpha = UInt32(bytes[index + 3])
        // Premultiplied as drawn; the average wants the colour itself.
        let channel = { (value: UInt8) -> UInt32 in
            alpha == 0 ? 0 : min(255, UInt32(value) * 255 / alpha)
        }
        return alpha << 24 | channel(bytes[index]) << 16 | channel(bytes[index + 1]) << 8 | channel(bytes[index + 2])
    }
    return pictureMood(pixels: pixels)
}

extension Scheme {
    /// The page colour tinted by a picture's `mood`, for the veils over that
    /// picture — the shared `appearance::mood_veil`. The page colour itself
    /// until the mood is known.
    func veil(_ mood: UInt32?) -> Color {
        guard let mood else { return background }
        return argb(moodVeil(background: backgroundArgb, mood: mood, light: light))
    }
}

/// A title's art across the top of a page: its backdrop, or with none, its
/// poster as a wash of its own colours — blurred, so it reads as the poster's
/// light rather than a blown-up slice of it. The desktop's `Fit::Ambient`.
struct Backdrop: View {
    let title: TitleRecord
    var onMood: ((UInt32) -> Void)?

    var body: some View {
        let wash = title.backdropUrl == nil && title.posterUrl != nil
        RemoteArtwork(
            title.backdropUrl ?? title.posterUrl,
            title.displayName,
            fallback: title.thumbnailSource,
            labelled: false,
            onMood: onMood
        )
            .blur(radius: wash ? 24 : 0, opaque: true)
    }
}

/// A title's logo — its name as lettering — in place of `fallback`, once it
/// has loaded. `fallback` stands until then, and for good when there is none,
/// so a title without a logo looks as it always did and one with a logo only
/// gains.
///
/// Trimmed to what is not transparent before it is drawn: TVDB pads every
/// clear logo to a square, and the padding would otherwise decide its size
/// and place.
///
/// `maxWidth` and `maxHeight` bound the logo only; the fallback takes the room
/// it always did.
struct RemoteLogo<Fallback: View>: View {
    let url: String?
    let description: String
    let maxWidth: CGFloat
    let maxHeight: CGFloat
    let fallback: () -> Fallback
    @State private var logo: UIImage?

    init(
        _ url: String?,
        _ description: String,
        maxWidth: CGFloat,
        maxHeight: CGFloat,
        @ViewBuilder fallback: @escaping () -> Fallback
    ) {
        self.url = url
        self.description = description
        self.maxWidth = maxWidth
        self.maxHeight = maxHeight
        self.fallback = fallback
        _logo = State(initialValue: ArtworkLoader.shared.cachedLogo(url))
    }

    var body: some View {
        Group {
            if let logo {
                Image(uiImage: logo)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: maxWidth, maxHeight: maxHeight, alignment: .bottomLeading)
                    .accessibilityLabel(description)
                    .transition(.opacity)
            } else {
                fallback().transition(.opacity)
            }
        }
        .task(id: url) {
            if logo != nil { return }
            let loaded = await ArtworkLoader.shared.logo(url)
            if Task.isCancelled { return }
            withAnimation(Motion.arrive) { logo = loaded }
        }
    }
}

/// Inclusive pixel bounds, top-left origin.
struct PixelBounds: Equatable {
    let left: Int
    let top: Int
    let right: Int
    let bottom: Int
}

/// The bounds of the pixels in a row-major `alpha` plane of `width` that are
/// not (nearly) transparent; `nil` when none are. Below the threshold is
/// anti-aliasing haze, not lettering.
func opaqueBounds(alpha: [UInt8], width: Int) -> PixelBounds? {
    var left = Int.max, top = Int.max, right = -1, bottom = -1
    for (index, value) in alpha.enumerated() where value > 8 {
        let x = index % width, y = index / width
        left = min(left, x)
        right = max(right, x)
        top = min(top, y)
        bottom = max(bottom, y)
    }
    return right < 0 ? nil : PixelBounds(left: left, top: top, right: right, bottom: bottom)
}

/// `image` cropped to `opaqueBounds`, or itself when there is nothing to crop.
func trimTransparent(_ image: UIImage) -> UIImage {
    guard let cgImage = image.cgImage else { return image }
    let width = cgImage.width, height = cgImage.height
    var pixels = [UInt8](repeating: 0, count: width * height * 4)
    let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
        guard let context = CGContext(
            data: buffer.baseAddress,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return false }
        context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
        return true
    }
    guard drawn else { return image }
    let alpha = stride(from: 3, to: pixels.count, by: 4).map { pixels[$0] }
    guard let bounds = opaqueBounds(alpha: alpha, width: width),
          bounds != PixelBounds(left: 0, top: 0, right: width - 1, bottom: height - 1)
    else { return image }
    let rect = CGRect(
        x: bounds.left,
        y: bounds.top,
        width: bounds.right - bounds.left + 1,
        height: bounds.bottom - bounds.top + 1
    )
    guard let cropped = cgImage.cropping(to: rect) else { return image }
    return UIImage(cgImage: cropped, scale: image.scale, orientation: image.imageOrientation)
}

private enum ArtworkLoad {
    case pending
    case missing
    case loaded(UIImage)
}

/// How long things take to change: the desktop's `theme::motion`.
enum Motion {
    /// Artwork arriving over its skeleton.
    static let arrive = Animation.easeOut(duration: 0.24)
    /// One pass of a skeleton's highlight.
    static let shimmer = Animation.linear(duration: 1.4).repeatForever(autoreverses: false)
}

/// Something being loaded: the surface tint, with a soft highlight sweeping
/// across it on a diagonal.
///
/// A spinner per tile is a grid of spinners; a shape where the poster will be
/// says what is coming and where, and settles the layout before it arrives.
struct Skeleton: View {
    @Environment(\.scheme) private var scheme
    @State private var sweep: CGFloat = -1

    var body: some View {
        scheme.surfaceVariant
            .overlay {
                GeometryReader { geometry in
                    LinearGradient(
                        colors: [.clear, mix(scheme.surfaceVariant, scheme.onSurface, 0.07), .clear],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                    .frame(width: geometry.size.width * 1.2)
                    .offset(x: sweep * geometry.size.width)
                }
            }
            .clipped()
            .onAppear { withAnimation(Motion.shimmer) { sweep = 2 } }
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

    func cachedLogo(_ url: String?) -> UIImage? {
        guard let url else { return nil }
        return decoded.object(forKey: "logo:\(url)" as NSString)
    }

    /// A title's logo, trimmed to its lettering — see `trimTransparent`.
    func logo(_ url: String?) async -> UIImage? {
        guard let url else { return nil }
        if let image = cachedLogo(url) { return image }
        guard let image = await remote(url) else { return nil }
        let trimmed = trimTransparent(image)
        decoded.setObject(trimmed, forKey: "logo:\(url)" as NSString, cost: trimmed.cost)
        return trimmed
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
        // Refused before reading when the server says up front it is too big.
        if response.expectedContentLength > Int64(Self.maxBytes) { return nil }
        // An array sized once, not a `Data` grown a byte at a time: on an old
        // iPad, a screen of posters read that way was seconds of CPU.
        var bytes = [UInt8]()
        bytes.reserveCapacity(Int(min(max(response.expectedContentLength, 0), Int64(Self.maxBytes))))
        do {
            for try await byte in stream {
                bytes.append(byte)
                if bytes.count > Self.maxBytes { return nil }
            }
        } catch {
            return nil
        }
        return Data(bytes)
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
