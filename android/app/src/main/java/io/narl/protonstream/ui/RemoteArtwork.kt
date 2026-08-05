package io.narl.protonstream.ui

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URI
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.security.MessageDigest

/** HTTPS-only artwork loader backed by the app-private cache directory. */
@Composable
internal fun RemoteArtwork(url: String?, description: String, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    var image by remember(url) { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(url) {
        image = url?.let { requested ->
            withContext(Dispatchers.IO) {
                runCatching {
                    val uri = URI(requested)
                    require(uri.scheme.equals("https", ignoreCase = true)) { "Artwork must use HTTPS" }
                    val directory = context.cacheDir.resolve("metadata-art").apply { mkdirs() }
                    val digest = MessageDigest.getInstance("SHA-256")
                        .digest(requested.toByteArray()).joinToString("") { "%02x".format(it) }
                    val cached = directory.resolve("$digest.img")
                    val bytes = if (cached.isFile) cached.readBytes() else {
                        val connection = uri.toURL().openConnection() as HttpURLConnection
                        connection.connectTimeout = 10_000
                        connection.readTimeout = 10_000
                        connection.instanceFollowRedirects = true
                        val status = connection.responseCode
                        require(status in 200..299) { "Artwork request failed ($status)" }
                        require(connection.url.protocol.equals("https", ignoreCase = true)) {
                            "Artwork redirect must use HTTPS"
                        }
                        connection.inputStream.use(::readBounded).also { downloaded ->
                            require(downloaded.size <= MAX_ART_BYTES) { "Artwork is too large" }
                            val temporary = directory.resolve("$digest.part")
                            temporary.writeBytes(downloaded)
                            check(temporary.renameTo(cached)) { "Unable to cache artwork" }
                        }
                    }
                    BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
                }.getOrNull()
            }
        }
    }
    Box(modifier.background(MaterialTheme.colorScheme.surfaceVariant), contentAlignment = Alignment.Center) {
        image?.let {
            Image(it, description, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        } ?: Text("▶", style = MaterialTheme.typography.displaySmall)
    }
}

private const val MAX_ART_BYTES = 12 * 1024 * 1024

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
