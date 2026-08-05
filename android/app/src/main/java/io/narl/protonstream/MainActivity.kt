package io.narl.protonstream

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import android.os.Build
import android.app.PictureInPictureParams
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.IBinder
import android.util.Rational
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import io.narl.protonstream.ui.ProtonStreamApp
import io.narl.protonstream.ui.theme.ProtonStreamTheme
import io.narl.protonstream.playback.NativeMpvHost
import io.narl.protonstream.playback.PlaybackService
import androidx.compose.runtime.mutableStateOf
import io.narl.protonstream.settings.SettingsStore

class MainActivity : ComponentActivity() {
    private val playerHost = mutableStateOf<NativeMpvHost?>(null)
    private var bound = false
    private val playbackConnection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
            playerHost.value = (service as PlaybackService.PlaybackBinder).host
            bound = true
        }
        override fun onServiceDisconnected(name: ComponentName?) {
            playerHost.value = null
            bound = false
        }
    }
    private val notificationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* Downloads remain usable; Android suppresses their notifications if denied. */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        bindService(Intent(this, PlaybackService::class.java), playbackConnection, Context.BIND_AUTO_CREATE)
        if (
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        setContent {
            ProtonStreamTheme {
                ProtonStreamApp(playerHost.value)
            }
        }
    }

    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        if (playerHost.value?.hasActivePlayback == true && !isInPictureInPictureMode) {
            enterPictureInPictureMode(
                PictureInPictureParams.Builder().setAspectRatio(Rational(16, 9)).setAutoEnterEnabled(true).build(),
            )
        }
    }

    override fun onDestroy() {
        if (bound) unbindService(playbackConnection)
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
}
