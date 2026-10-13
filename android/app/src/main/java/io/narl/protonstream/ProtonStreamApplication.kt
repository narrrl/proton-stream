package io.narl.protonstream

import android.app.Application
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.settings.SettingsStore

class ProtonStreamApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        NativeRuntime.initialize(this)
        // SharedPreferences reads its file on first use, and the first use was
        // on the main thread — in the playback service's start callback among
        // others. Read here, off it, and every later read is from memory.
        Thread({ SettingsStore.preload(this) }, "settings-preload").start()
    }
}
