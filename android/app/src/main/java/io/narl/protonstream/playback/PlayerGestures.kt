package io.narl.protonstream.playback

import android.app.Activity
import android.content.Context
import android.media.AudioManager
import android.view.WindowManager
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material.icons.filled.BrightnessMedium
import androidx.compose.material.icons.filled.FastForward
import androidx.compose.material.icons.filled.FastRewind
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import kotlin.math.abs
import kotlin.math.roundToInt

/** What a swipe on the picture is doing, for the bubble that says so. */
sealed interface Swipe {
    /** The window's brightness, from 0 to 1. */
    data class Brightness(val level: Float) : Swipe

    /** The device's media volume, from 0 to 1. */
    data class Volume(val level: Float) : Swipe

    /** Where the playhead will land when the finger lifts, and where it was. */
    data class Seek(val from: Double, val to: Double) : Swipe
}

/** How much of the file one swipe across the whole width is worth, in seconds. */
private const val SEEK_SPAN_SECONDS = 90.0

/**
 * A finger that comes down this close to the top or bottom of the screen is
 * the system's: pulling down the shade, swiping home.
 */
private val SYSTEM_EDGE = 48.dp

/** Which way a swipe went, settled by its first movement past the touch slop. */
private enum class Axis { Brightness, Volume, Seek, Ignored }

/**
 * The swipes every phone video player has taught: up and down on the left for
 * brightness, on the right for volume, and sideways to seek — shown while the
 * finger is down and only seeking when it lifts, because a seek per movement
 * would throw away every block in flight.
 */
fun Modifier.playerSwipes(
    enabled: Boolean,
    activity: Activity?,
    position: () -> Double,
    duration: () -> Double,
    onSwipe: (Swipe?) -> Unit,
    onSeek: (Double) -> Unit,
): Modifier = pointerInput(enabled, activity) {
    if (!enabled) return@pointerInput
    val audio = activity?.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
    val edge = SYSTEM_EDGE.toPx()
    var axis = Axis.Ignored
    var start = Offset.Zero
    var travelled = Offset.Zero
    var level = 0f
    var seekFrom = 0.0
    var seekTo = 0.0
    detectDragGestures(
        onDragStart = { offset ->
            start = offset
            travelled = Offset.Zero
            axis = if (offset.y < edge || offset.y > size.height - edge) Axis.Ignored else Axis.Seek
        },
        onDragEnd = {
            if (axis == Axis.Seek && seekTo != seekFrom) onSeek(seekTo)
            onSwipe(null)
        },
        onDragCancel = { onSwipe(null) },
    ) { change, amount ->
        if (axis == Axis.Ignored) return@detectDragGestures
        change.consume()
        val first = travelled == Offset.Zero
        travelled += amount
        if (first) {
            // The first movement past the slop decides what the swipe is for.
            axis = when {
                abs(amount.x) > abs(amount.y) -> Axis.Seek
                start.x < size.width / 2 -> Axis.Brightness
                else -> Axis.Volume
            }
            level = when (axis) {
                Axis.Brightness -> activity?.brightness() ?: 0.5f
                Axis.Volume -> audio?.mediaLevel() ?: 0f
                else -> 0f
            }
            seekFrom = position()
            seekTo = seekFrom
        }
        when (axis) {
            // A swipe the whole height of the screen goes from none to all.
            Axis.Brightness -> {
                level = (level - amount.y / size.height).coerceIn(0f, 1f)
                activity?.setBrightness(level)
                onSwipe(Swipe.Brightness(level))
            }
            Axis.Volume -> {
                level = (level - amount.y / size.height).coerceIn(0f, 1f)
                audio?.setMediaLevel(level)
                onSwipe(Swipe.Volume(level))
            }
            Axis.Seek -> {
                val length = duration()
                if (length <= 0.0) return@detectDragGestures
                seekTo = (seekFrom + travelled.x / size.width * SEEK_SPAN_SECONDS).coerceIn(0.0, length)
                onSwipe(Swipe.Seek(seekFrom, seekTo))
            }
            Axis.Ignored -> {}
        }
    }
}

/** The window's own brightness, or the screen's when the window has none. */
private fun Activity.brightness(): Float {
    val own = window.attributes.screenBrightness
    if (own >= 0f) return own
    return runCatching {
        android.provider.Settings.System.getInt(contentResolver, android.provider.Settings.System.SCREEN_BRIGHTNESS) / 255f
    }.getOrDefault(0.5f)
}

private fun Activity.setBrightness(level: Float) {
    window.attributes = window.attributes.apply {
        // Never quite off: a black screen under a playing film looks broken.
        screenBrightness = level.coerceAtLeast(0.01f)
    }
}

/** Hand brightness back to the system, as the player is left. */
fun Activity.releaseBrightness() {
    window.attributes = window.attributes.apply {
        screenBrightness = WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE
    }
}

private fun AudioManager.mediaLevel(): Float {
    val max = getStreamMaxVolume(AudioManager.STREAM_MUSIC).coerceAtLeast(1)
    return getStreamVolume(AudioManager.STREAM_MUSIC).toFloat() / max
}

private fun AudioManager.setMediaLevel(level: Float) {
    val max = getStreamMaxVolume(AudioManager.STREAM_MUSIC)
    // No flags: the bubble is the feedback, and the system's own volume panel
    // over the picture would be a second one.
    setStreamVolume(AudioManager.STREAM_MUSIC, (level * max).roundToInt(), 0)
}

/** The bubble in the middle of the picture while a swipe is under way. */
@Composable
fun SwipeBubble(swipe: Swipe, format: (Double) -> String, modifier: Modifier = Modifier) {
    Column(
        modifier
            .clip(RoundedCornerShape(20.dp))
            .background(Color.Black.copy(alpha = 0.6f))
            .padding(horizontal = 24.dp, vertical = 16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        when (swipe) {
            is Swipe.Brightness -> Level(Icons.Default.BrightnessMedium, "Brightness", swipe.level)
            is Swipe.Volume -> Level(Icons.AutoMirrored.Filled.VolumeUp, "Volume", swipe.level)
            is Swipe.Seek -> {
                val delta = swipe.to - swipe.from
                Icon(
                    if (delta < 0) Icons.Default.FastRewind else Icons.Default.FastForward,
                    contentDescription = null,
                    tint = Color.White,
                    modifier = Modifier.size(32.dp),
                )
                Text(
                    (if (delta < 0) "−" else "+") + format(abs(delta)),
                    color = Color.White,
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(format(swipe.to), color = Color.White.copy(alpha = 0.7f), style = MaterialTheme.typography.labelMedium)
            }
        }
    }
}

@Composable
private fun Level(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, level: Float) {
    Icon(icon, contentDescription = null, tint = Color.White, modifier = Modifier.size(32.dp))
    Text("$label ${(level * 100).roundToInt()}%", color = Color.White, style = MaterialTheme.typography.labelLarge)
    Box(Modifier.width(120.dp)) {
        LinearProgressIndicator(
            progress = { level },
            modifier = Modifier.fillMaxWidth().height(4.dp),
            color = MaterialTheme.colorScheme.primary,
            trackColor = Color.White.copy(alpha = 0.25f),
            drawStopIndicator = {},
        )
    }
}
