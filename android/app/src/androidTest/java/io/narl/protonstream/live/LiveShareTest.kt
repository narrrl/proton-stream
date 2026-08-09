package io.narl.protonstream.live

import android.graphics.SurfaceTexture
import android.view.Surface
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.narl.protonstream.playback.NativeMpvHost
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.BeforeClass
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.pstr_android.AndroidEngine
import uniffi.pstr_android.AndroidPaths
import uniffi.pstr_android.AndroidSecretStore
import uniffi.pstr_android.DownloadObserver
import uniffi.pstr_android.EpisodeRecord

/**
 * The cases `docs/ANDROID.md` requires that cannot be answered without a real
 * share: that a link resolves into a playable catalog, that libmpv actually
 * decodes the decrypted stream, that a cancelled download leaves a resumable
 * `.part`, and that a resume position survives a restart.
 *
 * The link is an instrumentation argument rather than a constant or a file, so
 * the secret never enters the repository:
 *
 * ```
 * -Pandroid.testInstrumentationRunnerArguments.pstr.shareUrl=...
 * ```
 *
 * Without it every test here is skipped by assumption, and
 * `scripts/android-acceptance.sh` reports the matrix as incomplete.
 *
 * Read-only against the share by construction: nothing here uploads, renames or
 * deletes anything remote. It writes only to this test's own temporary
 * directories.
 */
@RunWith(AndroidJUnit4::class)
class LiveShareTest {
    @Test
    fun theShareResolvesIntoAPlayableCatalog() {
        val episodes = episodes()
        assertTrue("the crawl found no playable file in the share", episodes.isNotEmpty())
        // A file with no size cannot be streamed: the block layer needs the
        // length to place block boundaries at all.
        val sized = episodes.filter { (it.size ?: 0uL) > 0uL }
        assertTrue("no episode in the share reported a size", sized.isNotEmpty())
    }

    @Test
    fun openingAnEpisodeYieldsASeekableRevision() {
        val episode = playable()
        val stream = runBlocking {
            engine.openStream(episode.shareId, episode.volumeId, episode.linkId)
        }
        try {
            assertTrue("the opened revision has no length", stream.size() > 0uL)
            // Without a revision id the block cache cannot tell one revision of
            // a file from another, and a stale block would be served as fresh.
            assertTrue("the opened revision has no id", stream.revisionId().isNotBlank())
        } finally {
            runBlocking { engine.releaseStream(episode.shareId, episode.volumeId, episode.linkId) }
        }
    }

    /**
     * The whole pipeline in one assertion: Proton block storage, decryption,
     * the Rust stream C ABI, the JNI adapter and libmpv's demuxer. If the
     * position advances, real decrypted bytes reached the decoder.
     */
    @Test
    fun libmpvDecodesTheDecryptedStream() {
        val episode = playable()
        val texture = SurfaceTexture(0).apply { setDefaultBufferSize(1280, 720) }
        val surface = Surface(texture)
        val host = NativeMpvHost()
        val key = "${episode.shareId}:${episode.linkId}"
        try {
            host.attachSurface(surface)
            val stream = runBlocking {
                engine.openStream(episode.shareId, episode.volumeId, episode.linkId)
            }
            runBlocking { host.play(key, stream.nativeHandle(), stream.size()) }
            host.setMuted(true)
            host.setPaused(false)

            val playing = waitFor(PLAYBACK_TIMEOUT_MS) { host.state.value.position > 0.0 }
            assertTrue(
                "libmpv never advanced past zero: ${host.state.value}",
                playing,
            )
            // One host serves every episode in turn, so a reading that does not
            // name this one would resume the wrong file.
            assertEquals(key, host.state.value.media)
            assertTrue("libmpv reported no duration", host.state.value.duration > 0.0)
        } finally {
            host.stop()
            host.detachSurface()
            host.close()
            surface.release()
            texture.release()
            runBlocking { engine.releaseStream(episode.shareId, episode.volumeId, episode.linkId) }
        }
    }

    /**
     * Cancellation is polled between whole Proton blocks precisely so the bytes
     * already written stay usable. This checks the retained `.part` is counted
     * as partial rather than as a finished file, and that resuming continues
     * from it instead of starting over.
     */
    @Test
    fun aCancelledDownloadLeavesAResumablePartFile() {
        val episode = playable()
        val firstRun = CancelAfter(bytes = RESUME_THRESHOLD_BYTES)
        runCatching {
            runBlocking {
                engine.downloadEpisode(episode.shareId, episode.volumeId, episode.linkId, firstRun)
            }
        }
        assumeTrue(
            "the episode finished before cancellation could be observed; it is too small for this case",
            !firstRun.completed,
        )
        assertTrue("the download reported no progress at all", firstRun.observed > 0L)

        val retained = engine.storageUsage()
        assertTrue("a cancelled download retained no bytes", retained.partialBytes > 0uL)
        assertEquals("a cancelled download counted as a finished file", 0uL, retained.offlineCount)

        // Resuming must pick up where the .part ended rather than at zero.
        val secondRun = CancelAfter(bytes = retained.partialBytes.toLong() + RESUME_THRESHOLD_BYTES)
        runCatching {
            runBlocking {
                engine.downloadEpisode(episode.shareId, episode.volumeId, episode.linkId, secondRun)
            }
        }
        assertTrue(
            "resuming restarted from zero: first reading was ${secondRun.firstReading}, " +
                "but ${retained.partialBytes} bytes were already retained",
            secondRun.firstReading >= retained.partialBytes.toLong(),
        )

        runBlocking { engine.removeOfflineEpisode(episode.shareId, episode.linkId) }
        assertEquals(
            "removing the episode left partial bytes behind",
            0uL,
            engine.storageUsage().partialBytes,
        )
    }

    /** What the viewer sees as "continue watching" after the app is killed. */
    @Test
    fun aResumePositionSurvivesReopeningTheCatalog() {
        val episode = playable()
        engine.saveWatchState(episode.shareId, episode.linkId, RESUME_POSITION, null, false)

        val reopened = AndroidEngine(paths, secrets)
        val state = reopened.watchState(episode.shareId, episode.linkId)
        assertNotNull("the resume position did not survive a reopen", state)
        assertEquals(RESUME_POSITION, state!!.positionSecs, 0.001)
        assertEquals(false, state.watched)

        // The catalog view is what the UI reads, so it has to agree.
        val listed = reopened.library(null)
            .flatMap { it.seasons }
            .flatMap { it.episodes }
            .first { it.linkId == episode.linkId }
        assertEquals(RESUME_POSITION, listed.resumeAt ?: -1.0, 0.001)
    }

    /** Cancels once the download has written enough to prove a resume. */
    private class CancelAfter(private val bytes: Long) : DownloadObserver {
        private val cancelled = AtomicBoolean(false)
        private val progress = AtomicLong(0)
        private val first = AtomicLong(-1)

        val observed: Long get() = progress.get()
        val firstReading: Long get() = first.get()
        val completed: Boolean get() = !cancelled.get()

        override fun onProgress(downloaded: ULong, total: ULong) {
            first.compareAndSet(-1, downloaded.toLong())
            progress.set(downloaded.toLong())
            if (downloaded.toLong() >= bytes) cancelled.set(true)
        }

        override fun isCancelled(): Boolean = cancelled.get()
    }

    private fun episodes(): List<EpisodeRecord> =
        engine.library(null).flatMap { it.seasons }.flatMap { it.episodes }

    private fun playable(): EpisodeRecord =
        episodes().firstOrNull { (it.size ?: 0uL) > 0uL }
            ?: throw AssertionError("the share has no sized file to play")

    private fun waitFor(timeoutMs: Long, condition: () -> Boolean): Boolean {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (System.currentTimeMillis() < deadline) {
            if (condition()) return true
            Thread.sleep(200)
        }
        return false
    }

    /** Stands in for Keystore, so a bridge failure cannot be blamed on crypto. */
    private class MemorySecretStore : AndroidSecretStore {
        private val entries = ConcurrentHashMap<String, String>()

        override fun set(key: String, value: String) {
            entries[key] = value
        }

        override fun get(key: String): String? = entries[key]

        override fun delete(key: String) {
            entries.remove(key)
        }
    }

    companion object {
        private const val SHARE_URL_ARGUMENT = "pstr.shareUrl"
        private const val SHARE_PASSWORD_ARGUMENT = "pstr.sharePassword"

        /** Long enough for a cold connection, a crawl and a first block read. */
        private const val PLAYBACK_TIMEOUT_MS = 60_000L

        /** Enough written bytes to prove a resume, small enough to stay quick. */
        private const val RESUME_THRESHOLD_BYTES = 4L * 1024 * 1024

        private const val RESUME_POSITION = 137.5

        private lateinit var paths: AndroidPaths
        private lateinit var secrets: AndroidSecretStore
        private lateinit var engine: AndroidEngine

        @BeforeClass
        @JvmStatic
        fun openTheShare() {
            System.loadLibrary("pstr_android")
            val arguments = InstrumentationRegistry.getArguments()
            val url = arguments.getString(SHARE_URL_ARGUMENT)
            assumeTrue(
                "set -Pandroid.testInstrumentationRunnerArguments.$SHARE_URL_ARGUMENT to run the live cases",
                !url.isNullOrBlank(),
            )
            val password = arguments.getString(SHARE_PASSWORD_ARGUMENT)?.takeIf { it.isNotBlank() }

            val root = File.createTempFile("pstr-live", "").let {
                it.delete()
                it.mkdirs()
                it
            }
            paths = AndroidPaths(
                File(root, "config").absolutePath,
                File(root, "data").absolutePath,
                File(root, "cache").absolutePath,
            )
            secrets = MemorySecretStore()
            engine = AndroidEngine(paths, secrets)
            engine.addShare("acceptance", url!!, password)
            runBlocking { engine.crawl(null) }
        }
    }
}
