package io.narl.protonstream.ui

import android.content.res.Configuration
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.InputMode
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalInputModeManager
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex

/*
 * A remote, rather than a finger.
 *
 * Android TV runs this same APK, and a television has no touch screen: every
 * control has to be reachable with the D-pad, show where the focus is from
 * across a room, and open what a long press opens some other way. Most of that
 * helps a keyboard on a phone or a Chromebook just as much, which is why it is
 * keyed to how the app is being driven rather than to the device, except where a
 * control makes no sense on a television at all.
 */

/** Whether this is a television: watched from across a room, driven by a remote. */
@Composable
fun isTelevision(): Boolean = isTelevision(LocalConfiguration.current)

internal fun isTelevision(configuration: Configuration): Boolean =
    configuration.uiMode and Configuration.UI_MODE_TYPE_MASK == Configuration.UI_MODE_TYPE_TELEVISION

/**
 * Whether keys are driving the app right now — a remote, a keyboard — rather
 * than touch. Compose switches on the first key press and back on the first
 * touch, and only shows focus while this holds.
 */
@Composable
fun keyDriven(): Boolean = LocalInputModeManager.current.inputMode == InputMode.Keyboard

/**
 * Move focus here, and say whether it went.
 *
 * A requester whose node is not composed — a card that has just closed — throws
 * rather than answering, and a focus that cannot move is an ordinary outcome.
 */
fun FocusRequester.tryFocus(): Boolean =
    runCatching { requestFocus(FocusDirection.Enter) }.getOrDefault(false)

/**
 * Show from across a room which control has the focus.
 *
 * Material's own focus cue is a ten percent tint, which over a poster is no cue
 * at all. This draws an accent outline, and with [grow] scales artwork up a
 * little and over its neighbours, the way a television's own launcher does.
 * Goes before the control's `clickable`, whose focus it reports.
 */
fun Modifier.focusHighlight(
    shape: Shape = RoundedCornerShape(8.dp),
    grow: Boolean = false,
): Modifier = composed {
    var focused by remember { mutableStateOf(false) }
    val scale by animateFloatAsState(if (focused && grow) FOCUS_SCALE else 1f, label = "focus scale")
    val outline = MaterialTheme.colorScheme.primary
    this
        .zIndex(if (focused) 1f else 0f)
        .graphicsLayer {
            scaleX = scale
            scaleY = scale
        }
        .then(if (focused) Modifier.border(FOCUS_OUTLINE, outline, shape) else Modifier)
        .onFocusChanged { focused = it.hasFocus }
}

/**
 * Open with the remote's Menu key what a long press opens.
 *
 * Holding OK down already counts as a long press, but nothing tells a viewer
 * that; Menu is the key a remote has for exactly this.
 */
fun Modifier.onMenuKey(onMenu: () -> Unit): Modifier = onPreviewKeyEvent { event ->
    if (event.key == Key.Menu && event.type == KeyEventType.KeyUp) {
        onMenu()
        true
    } else {
        event.key == Key.Menu
    }
}

private const val FOCUS_SCALE = 1.06f
private val FOCUS_OUTLINE = 3.dp

/**
 * The title whose tile should take the focus back, and how to say it has.
 *
 * Set by the app when a title closes, read by the tile showing that title.
 */
class FocusReturn(val key: String?, val done: () -> Unit)

val LocalFocusReturn = compositionLocalOf<FocusReturn?> { null }

/**
 * Take the focus back if this is the tile [LocalFocusReturn] names.
 *
 * Only while keys drive the app: a touch has no focus to lose. Said done either
 * way, so a tile scrolled back into view later does not take it a second time.
 */
fun Modifier.returnsFocus(key: String): Modifier = composed {
    val back = LocalFocusReturn.current
    if (back?.key != key) return@composed Modifier
    val requester = remember { FocusRequester() }
    val byKeys = keyDriven()
    LaunchedEffect(Unit) {
        if (byKeys) requester.tryFocus()
        back.done()
    }
    Modifier.focusRequester(requester)
}
