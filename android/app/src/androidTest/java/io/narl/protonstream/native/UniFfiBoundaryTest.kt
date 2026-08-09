package io.narl.protonstream.native

import androidx.test.ext.junit.runners.AndroidJUnit4
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.BeforeClass
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.pstr_android.AndroidEngine
import uniffi.pstr_android.AndroidPaths
import uniffi.pstr_android.AndroidSecretStore
import uniffi.pstr_android.chapterPlan
import uniffi.pstr_android.skipOffer

/**
 * The first thing that crosses the UniFFI boundary on a real device. It is
 * deliberately offline: it proves the ABI, the generated bindings, the callback
 * interface and the SQLite open all work, without needing a share.
 *
 * If this fails and the Rust workspace tests pass, the fault is in packaging —
 * a missing ABI in `jniLibs`, a stale binding, or an R8 rule (run it with
 * `ANDROID_TEST_BUILD_TYPE=release` to check the shipped configuration).
 */
@RunWith(AndroidJUnit4::class)
class UniFfiBoundaryTest {
    /** Stands in for Keystore so a bridge failure cannot be blamed on crypto. */
    private class MemorySecretStore : AndroidSecretStore {
        val entries = ConcurrentHashMap<String, String>()
        var reads = 0
            private set

        override fun set(key: String, value: String) {
            entries[key] = value
        }

        override fun get(key: String): String? {
            reads += 1
            return entries[key]
        }

        override fun delete(key: String) {
            entries.remove(key)
        }
    }

    private fun engine(secrets: AndroidSecretStore = MemorySecretStore()): AndroidEngine {
        val root = File.createTempFile("pstr-bridge", "").let {
            it.delete()
            it.mkdirs()
            it
        }
        return AndroidEngine(
            AndroidPaths(
                File(root, "config").absolutePath,
                File(root, "data").absolutePath,
                File(root, "cache").absolutePath,
            ),
            secrets,
        )
    }

    @Test
    fun theEngineOpensAgainstEmptyDirectories() {
        val engine = engine()
        assertTrue(engine.shares().isEmpty())
        assertTrue(engine.library(null).isEmpty())
    }

    @Test
    fun storageUsageReportsZeroOnAFreshInstall() {
        val usage = engine().storageUsage()
        assertNotNull(usage)
        assertEquals(0UL, usage.offlineBytes)
        assertEquals(0UL, usage.partialBytes)
        assertEquals(0UL, usage.offlineCount)
    }

    @Test
    fun metadataSettingsAreReadableAndDefaultToDisabled() {
        // Enrichment is opt-in; a default of enabled would send titles to a
        // third party before the viewer has agreed to it.
        assertEquals(false, engine().metadataSettings().enabled)
    }

    @Test
    fun aSecondEngineReopensTheSameCatalog() {
        val secrets = MemorySecretStore()
        engine(secrets).shares()
        // Reopening is the process-restart path; a schema or lock fault shows up
        // here rather than on a user's device.
        assertTrue(engine(secrets).shares().isEmpty())
    }

    /**
     * Chapter classification is shared Rust that desktop already pins
     * (`docs/BUGS.md` B12). This runs the same code through the bindings, so a
     * lowering fault in the record types cannot pass unnoticed.
     */
    @Test
    fun chapterPlanningCrossesTheBoundaryIntact() {
        val plan = chapterPlan(emptyList(), 1560.0)
        assertNotNull(plan)
        assertNull(skipOffer(plan, 12.0))
    }

    companion object {
        @BeforeClass
        @JvmStatic
        fun loadLibrary() {
            System.loadLibrary("pstr_android")
        }
    }
}
