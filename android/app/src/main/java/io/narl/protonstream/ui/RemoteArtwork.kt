package io.narl.protonstream.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import io.narl.protonstream.native.NativeRuntime
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.TitleRecord
import java.net.HttpURLConnection
import java.net.URI
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.InputStream
import java.security.MessageDigest
import java.util.Collections

/**
 * One file to pull Proton's own thumbnail from when no provider poster exists.
 *
 * Proton renders a thumbnail per file, so any episode of a show is a frame of
 * the right show — the desktop client picks the first for the same reason.
 */
data class ThumbnailSource(val shareId: String, val volumeId: String, val linkId: String) {
    val key: String get() = "$shareId/$linkId"
}

/**
 * What the first episode of a title offers as a poster.
 *
 * Metadata lookups are off by default, so for a freshly crawled library this is
 * the only artwork there is.
 */
internal val TitleRecord.thumbnailSource: ThumbnailSource?
    get() = seasons.firstNotNullOfOrNull { season -> season.episodes.firstOrNull() }
        ?.let { ThumbnailSource(it.shareId, it.volumeId, it.linkId) }

/** What one episode offers as a still. */
internal val EpisodeRecord.thumbnailSource: ThumbnailSource
    get() = ThumbnailSource(shareId, volumeId, linkId)

/**
 * HTTPS-only artwork loader backed by the app-private cache directory.
 *
 * [fallback] is used only when there is no provider artwork or it fails to
 * load, which — with metadata off — is the common case rather than the rare
 * one. Until either answers, the box pulses as a skeleton; when neither has
 * anything, it shows [ArtworkPlaceholder] rather than an empty card — without
 * initials when [labelled] is false, for art with the name already over it.
 */
@Composable
internal fun RemoteArtwork(
    url: String?,
    description: String,
    modifier: Modifier = Modifier,
    fallback: ThumbnailSource? = null,
    labelled: Boolean = true,
) {
    val context = LocalContext.current
    val enabled = LocalArtworkLoading.current
    var image by remember(url, fallback) { mutableStateOf<ArtworkLoad>(ArtworkLoad.Pending) }
    LaunchedEffect(url, fallback, enabled) {
        if (!enabled) {
            image = ArtworkLoad.Missing
            return@LaunchedEffect
        }
        image = (loadArtwork(context, url) ?: loadThumbnail(fallback))
            ?.let { ArtworkLoad.Loaded(it.asImageBitmap()) }
            ?: ArtworkLoad.Missing
    }
    when (val loaded = image) {
        ArtworkLoad.Pending -> Skeleton(modifier)
        ArtworkLoad.Missing -> ArtworkPlaceholder(description, modifier, labelled)
        is ArtworkLoad.Loaded -> Image(loaded.image, description, modifier, contentScale = ContentScale.Crop)
    }
}

private sealed interface ArtworkLoad {
    data object Pending : ArtworkLoad
    data object Missing : ArtworkLoad
    data class Loaded(val image: ImageBitmap) : ArtworkLoad
}

/**
 * Whether [RemoteArtwork] fetches anything at all.
 *
 * Off in the screenshot tests: there is no bridge on the host, and whether a
 * failed fetch lands before or after the capture would decide between a
 * skeleton and a placeholder in the committed image.
 */
internal val LocalArtworkLoading = staticCompositionLocalOf { true }

/**
 * Something being loaded: the surface tint, breathing.
 *
 * A spinner per tile is a grid of spinners; a shape where the poster will be
 * says what is coming and where, and settles the layout before it arrives.
 */
@Composable
internal fun Skeleton(modifier: Modifier = Modifier) {
    val pulse by rememberInfiniteTransition(label = "skeleton").animateFloat(
        initialValue = 0.55f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(800), RepeatMode.Reverse),
        label = "skeleton alpha",
    )
    Box(modifier.graphicsLayer { alpha = pulse }.background(MaterialTheme.colorScheme.surfaceVariant))
}

/**
 * Art for a title that has none: the accent as a gradient, and its initials.
 *
 * A black card with a play triangle on it said nothing about which title it
 * was, so a grid of unmatched titles was a grid of identical cards. Each name
 * sets where on the accent ramp its gradient starts, so neighbours differ
 * while every one stays in the theme's colours.
 */
@Composable
internal fun ArtworkPlaceholder(name: String, modifier: Modifier = Modifier, labelled: Boolean = true) {
    val scheme = MaterialTheme.colorScheme
    val brush = remember(name, scheme.primary, scheme.secondary) {
        val start = lerp(scheme.primary, scheme.secondary, (name.hashCode().mod(5)) / 4f)
        Brush.linearGradient(listOf(start, lerp(start, Color.Black, 0.55f)))
    }
    BoxWithConstraints(modifier.background(brush), contentAlignment = Alignment.Center) {
        if (!labelled) return@BoxWithConstraints
        val size = with(LocalDensity.current) { (minOf(maxWidth, maxHeight) * 0.3f).toSp() }
        Text(
            initials(name),
            color = Color.White.copy(alpha = 0.9f),
            fontSize = size,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
        )
    }
}

/**
 * "Oshi no Ko" is "OK", "Akira" is "A": the first letters of up to two
 * capitalised words, so particles and articles in a romanised name do not
 * take a slot. A name with no capitals uses its first two words.
 */
internal fun initials(name: String): String {
    val words = name.split(' ', '-', '_', '.', ':').filter { word -> word.firstOrNull()?.isLetterOrDigit() == true }
    val picked = words.filter { it.first().isUpperCase() || it.first().isDigit() }.ifEmpty { words }
    return picked.take(2).joinToString("") { it.first().uppercase() }
}

/**
 * The same HTTPS-only fetch behind the library grid, reusable off the composition.
 *
 * The media notification needs a bitmap rather than an [ImageBitmap], and needs
 * it from a service with no composition of its own — but it wants exactly this
 * loader's protocol checks and exactly this disk cache, so the artwork the shade
 * shows costs no second download.
 */
internal suspend fun loadArtwork(context: Context, url: String?): Bitmap? = url?.let { requested ->
    decoded.get(requested) ?: withContext(Dispatchers.IO) {
        runCatching {
            val uri = URI(requested)
            require(uri.scheme.equals("https", ignoreCase = true)) { "Artwork must use HTTPS" }
            val directory = context.cacheDir.resolve("metadata-art").apply { mkdirs() }
            val digest = MessageDigest.getInstance("SHA-256")
                .digest(requested.toByteArray()).joinToString("") { "%02x".format(it) }
            val cached = directory.resolve("$digest.img")
            val fresh = !cached.isFile
            val bytes = if (fresh) download(uri) else cached.readBytes()
            val bitmap = requireNotNull(decodeSampled(bytes)) { "Artwork is not a decodable image" }
            if (fresh) {
                // Written only once it has decoded. Caching the bytes first is
                // what turns one hostile poster into a crash on every launch
                // that survives until app data is cleared.
                val temporary = directory.resolve("$digest.part")
                temporary.writeBytes(bytes)
                check(temporary.renameTo(cached)) { "Unable to cache artwork" }
                prune(directory)
            } else {
                cached.setLastModified(System.currentTimeMillis())
            }
            bitmap
        }.getOrNull()?.also { decoded.put(requested, it) }
    }
}

/**
 * Proton's own thumbnail for one file, decoded.
 *
 * The bridge holds the disk cache, so what is worth keeping here is the decoded
 * bitmap and the *absence* of one: a file with no thumbnail is common, and
 * without remembering that, every scroll back over the grid pays a round trip
 * per tile to be told so again. A failed call is not remembered — an unreachable
 * share or a session that has to be reopened is transient, and the next
 * composition should try again.
 */
internal suspend fun loadThumbnail(source: ThumbnailSource?): Bitmap? {
    val file = source ?: return null
    val key = file.key
    decoded.get(key)?.let { return it }
    if (key in missing) return null
    val fetched = withContext(Dispatchers.IO) {
        runCatching { NativeRuntime.engine().thumbnail(file.shareId, file.volumeId, file.linkId) }
    }
    val bytes = fetched.getOrElse { return null }
    val bitmap = bytes?.let { withContext(Dispatchers.Default) { decodeSampled(it) } }
    if (bitmap == null) {
        missing.add(key)
        return null
    }
    decoded.put(key, bitmap)
    return bitmap
}

private fun download(uri: URI): ByteArray {
    val connection = uri.toURL().openConnection() as HttpURLConnection
    connection.connectTimeout = 10_000
    connection.readTimeout = 10_000
    connection.instanceFollowRedirects = true
    val status = connection.responseCode
    require(status in 200..299) { "Artwork request failed ($status)" }
    require(connection.url.protocol.equals("https", ignoreCase = true)) {
        "Artwork redirect must use HTTPS"
    }
    return connection.inputStream.use(::readBounded)
        .also { require(it.size <= MAX_ART_BYTES) { "Artwork is too large" } }
}

/**
 * Decode within a bound the display can actually use.
 *
 * [MAX_ART_BYTES] bounds the *compressed* image, which says nothing about what
 * it costs decoded: a 12 MiB PNG can be 12000×12000, and that is a 576 MB
 * `ARGB_8888` allocation on a device that will not give it. The bounds pass
 * costs no allocation and settles a sample size before anything is committed.
 */
private fun decodeSampled(bytes: ByteArray): Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    val longest = maxOf(bounds.outWidth, bounds.outHeight)
    if (longest <= 0) return null
    var sample = 1
    while (longest / sample > MAX_ART_EDGE) sample *= 2
    return BitmapFactory.decodeByteArray(
        bytes,
        0,
        bytes.size,
        BitmapFactory.Options().apply { inSampleSize = sample },
    )
}

/**
 * Hold the cache directory under [MAX_ART_CACHE_BYTES], oldest first.
 *
 * `cacheDir` is reclaimable by the system, but only under storage pressure and
 * only wholesale — a poster cache that grows with every title ever browsed is
 * still the app's own doing.
 */
private fun prune(directory: File) {
    val files = directory.listFiles()?.sortedBy(File::lastModified) ?: return
    var total = files.sumOf(File::length)
    for (file in files) {
        if (total <= MAX_ART_CACHE_BYTES) return
        total -= file.length()
        file.delete()
    }
}

/**
 * Files known to have no Proton thumbnail, so the answer is paid for once.
 *
 * Deliberately not persisted: a re-upload can add one, and the cost of being
 * wrong for a session is a placeholder that a restart clears.
 */
private val missing: MutableSet<String> = Collections.synchronizedSet(mutableSetOf())

/**
 * Decoded posters, so scrolling the grid re-reads neither disk nor decoder.
 *
 * Keyed by artwork URL or by `share/link` for a Proton thumbnail; the two
 * cannot collide, since one is always an `https:` URL.
 *
 * `remember(url)` is per-composition, and a `LazyVerticalGrid` drops and
 * recreates items as they leave the viewport — without this, every scroll back
 * is a full-resolution decode per item.
 */
private val decoded = object : LruCache<String, Bitmap>(
    (Runtime.getRuntime().maxMemory() / 8).coerceIn(4L * 1024 * 1024, 32L * 1024 * 1024).toInt(),
) {
    override fun sizeOf(key: String, value: Bitmap): Int = value.byteCount
}

private const val MAX_ART_BYTES = 12 * 1024 * 1024
private const val MAX_ART_CACHE_BYTES = 48L * 1024 * 1024

/** Longest edge worth decoding: a poster on a phone, or a notification icon. */
private const val MAX_ART_EDGE = 1_024

/** Reads at most [MAX_ART_BYTES] plus one byte without requiring API 33's readNBytes. */
private fun readBounded(input: InputStream): ByteArray {
    val output = ByteArrayOutputStream(MAX_ART_BYTES.coerceAtMost(64 * 1024))
    val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
    var total = 0
    while (total <= MAX_ART_BYTES) {
        val requested = minOf(buffer.size, MAX_ART_BYTES + 1 - total)
        val count = input.read(buffer, 0, requested)
        if (count < 0) break
        if (count == 0) continue
        output.write(buffer, 0, count)
        total += count
    }
    return output.toByteArray()
}
