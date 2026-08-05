package io.narl.protonstream.native

import android.content.Context
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.pstr_android.AndroidEngine
import uniffi.pstr_android.AndroidPaths

/** Process-wide owner of the Rust engine; records and decrypted media remain app-private. */
object NativeRuntime {
    private lateinit var appContext: Context
    @Volatile private var instance: AndroidEngine? = null

    fun initialize(context: Context) {
        System.loadLibrary("pstr_android")
        initTls(context)
        appContext = context.applicationContext
    }

    /** Initializes Android's certificate verifier before Proton networking. */
    private external fun initTls(context: Context)

    suspend fun engine(): AndroidEngine = withContext(Dispatchers.IO) {
        instance ?: synchronized(this@NativeRuntime) {
            instance ?: createEngine().also { instance = it }
        }
    }

    private fun createEngine(): AndroidEngine {
        check(::appContext.isInitialized) { "NativeRuntime is not initialized" }
        val config = File(appContext.noBackupFilesDir, "config")
        val data = File(appContext.filesDir, "data")
        val cache = File(appContext.cacheDir, "stream")
        return AndroidEngine(
            AndroidPaths(config.absolutePath, data.absolutePath, cache.absolutePath),
            KeystoreSecretStore(appContext),
        )
    }
}
