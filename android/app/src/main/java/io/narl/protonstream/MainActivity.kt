package io.narl.protonstream

import android.Manifest
import android.app.PendingIntent
import android.app.PictureInPictureParams
import android.app.RemoteAction
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Rect
import android.graphics.drawable.Icon
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import android.util.Rational
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.snapshotFlow
import androidx.core.content.ContextCompat
import androidx.core.graphics.drawable.toDrawable
import androidx.core.view.WindowCompat
import androidx.lifecycle.lifecycleScope
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.playback.MpvPlaybackState
import io.narl.protonstream.playback.NativeMpvHost
import io.narl.protonstream.playback.PlaybackService
import io.narl.protonstream.settings.SettingsStore
import io.narl.protonstream.ui.ProtonStreamApp
import io.narl.protonstream.ui.theme.AppearanceState
import io.narl.protonstream.ui.theme.ProtonStreamTheme
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import uniffi.pstr_android.PaletteRecord

class MainActivity : ComponentActivity() {
    private val playerHost = mutableStateOf<NativeMpvHost?>(null)
    private val inPictureInPicture = mutableStateOf(false)

    /** A share link handed over by a browser or the share sheet, until the Add form takes it. */
    private val incomingShareLink = mutableStateOf<String?>(null)
    /**
     * Whether a bind is outstanding, which is what has to be unbound.
     *
     * Set when `bindService` is called rather than when the connection arrives:
     * an activity destroyed while the bind is still pending owes the system an
     * `unbindService` just the same, and `onServiceConnected` may never run.
     */
    private var bound = false
    private val playbackConnection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
            playerHost.value = (service as PlaybackService.PlaybackBinder).host
        }
        override fun onServiceDisconnected(name: ComponentName?) {
            playerHost.value = null
        }
    }

    /**
     * The transport behind the buttons Android draws over the PiP window.
     *
     * A broadcast rather than a service intent: PiP is one of the states in
     * which a background service start is refused, and the receiver is
     * registered for as long as the activity that owns the window exists.
     */
    private val pictureInPictureControls = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            val host = playerHost.value ?: return
            when (intent?.getStringExtra(EXTRA_CONTROL)) {
                CONTROL_PLAY -> host.setPaused(false)
                CONTROL_PAUSE -> host.setPaused(true)
                CONTROL_NEXT -> host.onSkipNext?.invoke()
                CONTROL_PREVIOUS -> host.onSkipPrevious?.invoke()
            }
        }
    }

    private val notificationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* Downloads remain usable; Android suppresses their notifications if denied. */ }

    @OptIn(ExperimentalCoroutinesApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Before anything draws. The palette decides the window's own
        // background and which way round the system bars' icons go, and both
        // are wrong for a frame if they are set from a composition instead.
        // Infallible by construction — an unreadable theme file resolves to the
        // shipped default rather than throwing.
        runCatching { NativeRuntime.storedPalette() }.onSuccess(AppearanceState::seed)
        // Only on a fresh start: a recreated activity is handed the intent it
        // was launched with again, and would offer to add the same share twice.
        if (savedInstanceState == null) incomingShareLink.value = intent.shareLink()
        enableEdgeToEdge()
        // Re-applied on every change, not just at startup: picking Latte from
        // the settings page has to turn the status bar's icons dark in the same
        // frame the page behind them turns light.
        lifecycleScope.launch {
            AppearanceState.palette.filterNotNull().distinctUntilChanged().collect(::dressWindow)
        }
        bound = bindService(
            Intent(this, PlaybackService::class.java),
            playbackConnection,
            Context.BIND_AUTO_CREATE,
        )
        ContextCompat.registerReceiver(
            this,
            pictureInPictureControls,
            IntentFilter(ACTION_PIP_CONTROL),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        if (
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        // Auto-enter has to be armed *before* the gesture that would use it:
        // building the params inside onUserLeaveHint is too late for the swipe
        // home that gesture navigation sends, which never calls it. Re-register
        // whenever the picture's shape or the transport's does.
        lifecycleScope.launch {
            snapshotFlow { playerHost.value }
                .flatMapLatest { host -> host?.state ?: flowOf(null) }
                .map { state -> PictureInPictureShape.of(state) }
                .distinctUntilChanged()
                .collect { shape -> runCatching { setPictureInPictureParams(shape.params()) } }
        }
        setContent {
            ProtonStreamTheme {
                ProtonStreamApp(
                    playerHost.value,
                    inPictureInPicture.value,
                    shareLink = incomingShareLink.value,
                    onShareLinkTaken = { incomingShareLink.value = null },
                )
            }
        }
    }

    /**
     * The parts of the window Compose does not reach: the background behind the
     * whole activity, and the polarity of the system bars' icons.
     *
     * The background matters for the frames before and between compositions —
     * the launch frame, and a rotation. It used to be a hex literal in
     * `themes.xml`, which meant every cold start showed a colour from one
     * flavour regardless of which one was chosen.
     *
     * The icons matter because the app ships a light flavour. Pinned dark, as
     * they were, Latte drew white status-bar icons onto a near-white page.
     */
    private fun dressWindow(palette: PaletteRecord) {
        window.setBackgroundDrawable(palette.background.toInt().toDrawable())
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = palette.light
            isAppearanceLightNavigationBars = palette.light
        }
    }

    // singleTask: a link opened while the app is running arrives here, not in
    // a second activity.
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.shareLink()?.let { incomingShareLink.value = it }
    }

    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        // The backstop for launchers that still send this instead of arming
        // auto-enter. Entering twice is not an error; not entering at all is.
        if (playerHost.value?.hasActivePlayback == true && !isInPictureInPictureMode) {
            runCatching {
                enterPictureInPictureMode(PictureInPictureShape.of(playerHost.value?.state?.value).params())
            }
        }
    }

    override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: Configuration) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        // The window is now a thumbnail with its own controls. Anything the
        // player would draw over it — the transport, the title bar, the skip
        // offer — is unreadable at that size and covers the picture.
        inPictureInPicture.value = isInPictureInPictureMode
    }

    override fun onDestroy() {
        runCatching { unregisterReceiver(pictureInPictureControls) }
        if (bound) {
            unbindService(playbackConnection)
            bound = false
        }
        super.onDestroy()
    }

    override fun onStop() {
        super.onStop()
        if (
            !isChangingConfigurations &&
            !isInPictureInPictureMode &&
            !SettingsStore(this).backgroundAudio
        ) {
            playerHost.value?.stop()
        }
    }

    /** Everything the PiP window's shape depends on, and nothing that polls. */
    private data class PictureInPictureShape(
        val active: Boolean,
        val paused: Boolean,
        val width: Int,
        val height: Int,
    ) {
        companion object {
            fun of(state: MpvPlaybackState?): PictureInPictureShape = PictureInPictureShape(
                active = state != null && state.duration > 0.0 && !state.ended,
                paused = state?.paused != false,
                width = state?.videoWidth?.toInt() ?: 0,
                height = state?.videoHeight?.toInt() ?: 0,
            )
        }
    }

    private fun PictureInPictureShape.params(): PictureInPictureParams =
        PictureInPictureParams.Builder()
            .setAspectRatio(aspectRatio(width, height))
            .setAutoEnterEnabled(active)
            .setSeamlessResizeEnabled(false)
            // The player owns the whole window, so the picture's rectangle is
            // the window's. Without it the transition into PiP cross-fades from
            // a screenshot instead of shrinking the video that is already there.
            .setSourceRectHint(windowBounds())
            .setActions(
                listOf(
                    remoteAction(android.R.drawable.ic_media_previous, "Previous episode", CONTROL_PREVIOUS, 11),
                    if (paused) remoteAction(android.R.drawable.ic_media_play, "Play", CONTROL_PLAY, 12)
                    else remoteAction(android.R.drawable.ic_media_pause, "Pause", CONTROL_PAUSE, 13),
                    remoteAction(android.R.drawable.ic_media_next, "Next episode", CONTROL_NEXT, 14),
                ),
            )
            .build()

    private fun windowBounds(): Rect = window.decorView.let { Rect(0, 0, it.width, it.height) }

    private fun remoteAction(icon: Int, label: String, control: String, requestCode: Int): RemoteAction =
        RemoteAction(
            Icon.createWithResource(this, icon),
            label,
            label,
            PendingIntent.getBroadcast(
                this,
                requestCode,
                Intent(ACTION_PIP_CONTROL).setPackage(packageName).putExtra(EXTRA_CONTROL, control),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            ),
        )

    private companion object {
        const val ACTION_PIP_CONTROL = "io.narl.protonstream.PIP_CONTROL"
        const val EXTRA_CONTROL = "control"
        const val CONTROL_PLAY = "play"
        const val CONTROL_PAUSE = "pause"
        const val CONTROL_NEXT = "next"
        const val CONTROL_PREVIOUS = "previous"

        /**
         * Android refuses a window narrower than 1:2.39 or taller than 2.39:1,
         * and refuses the request outright rather than clamping it — so a
         * 2.40:1 scope release would take PiP away entirely.
         */
        fun aspectRatio(width: Int, height: Int): Rational {
            if (width <= 0 || height <= 0) return Rational(16, 9)
            val ratio = width.toDouble() / height
            return when {
                ratio > MAX_RATIO -> Rational(239, 100)
                ratio < 1 / MAX_RATIO -> Rational(100, 239)
                else -> Rational(width, height)
            }
        }

        const val MAX_RATIO = 2.39
    }
}
