package io.narl.protonstream

import android.app.Application
import io.narl.protonstream.native.NativeRuntime

class ProtonStreamApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        NativeRuntime.initialize(this)
    }
}
