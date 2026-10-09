package io.narl.protonstream

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.mutableStateOf
import androidx.core.graphics.drawable.toDrawable
import androidx.core.view.WindowCompat
import androidx.lifecycle.lifecycleScope
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.playback.NativeMpvHost
import io.narl.protonstream.playback.PlaybackService
import io.narl.protonstream.settings.SettingsStore
import io.narl.protonstream.sync.WatchSyncWorker
import io.narl.protonstream.ui.ProtonStreamApp
import io.narl.protonstream.ui.theme.AppearanceState
import io.narl.protonstream.ui.theme.ProtonStreamTheme
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.launch
import uniffi.pstr_android.PaletteRecord

class MainActivity : ComponentActivity() {
    private val playerHost = mutableStateOf<NativeMpvHost?>(null)

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

    private val notificationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* Downloads remain usable; Android suppresses their notifications if denied. */ }

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
        if (
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        setContent {
            ProtonStreamTheme {
                ProtonStreamApp(
                    playerHost.value,
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

    override fun onDestroy() {
        if (bound) {
            unbindService(playbackConnection)
            bound = false
        }
        super.onDestroy()
    }

    override fun onStop() {
        super.onStop()
        // Where the viewer stopped is what their other devices want next, and
        // a process in the background may not live to the next timed sync.
        if (!isChangingConfigurations) WatchSyncWorker.enqueue(this)
        if (
            !isChangingConfigurations &&
            !SettingsStore(this).backgroundAudio
        ) {
            playerHost.value?.stop()
        }
    }
}
