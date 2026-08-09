package io.narl.protonstream.native

import android.content.Context
import android.util.Base64
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.security.KeyStore
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.pstr_android.BridgeException

/**
 * Runs on a device because Android Keystore has no host implementation — which
 * is why nothing pinned this before. The share fragment and any custom link
 * password are the only user secrets the app holds, and losing the key makes
 * every share unopenable, so the failure modes matter as much as the happy path.
 */
@RunWith(AndroidJUnit4::class)
class KeystoreSecretStoreTest {
    private lateinit var context: Context
    private lateinit var store: KeystoreSecretStore

    @Before
    fun setUp() {
        context = ApplicationProvider.getApplicationContext()
        clearPreferences()
        store = KeystoreSecretStore(context)
    }

    @After
    fun tearDown() {
        clearPreferences()
    }

    @Test
    fun aSecretSurvivesARoundTrip() {
        store.set("share-1", "correct horse battery staple")
        assertEquals("correct horse battery staple", store.get("share-1"))
    }

    @Test
    fun anAbsentSecretIsNullRatherThanEmpty() {
        assertNull(store.get("never-written"))
    }

    @Test
    fun deleteRemovesTheSecret() {
        store.set("share-1", "value")
        store.delete("share-1")
        assertNull(store.get("share-1"))
    }

    @Test
    fun aUnicodeSecretIsNotMangled() {
        val value = "パスワード・🎬・ü"
        store.set("share-1", value)
        assertEquals(value, store.get("share-1"))
    }

    /**
     * GCM is catastrophic under IV reuse: two secrets encrypted with the same
     * key and IV leak the XOR of their plaintexts. `Cipher.init(ENCRYPT_MODE)`
     * is what generates a fresh one, so this pins that the code never pins it.
     */
    @Test
    fun everyWriteUsesAFreshInitialisationVector() {
        store.set("share-1", "same plaintext")
        val first = rawPayload("share-1")
        store.set("share-1", "same plaintext")
        val second = rawPayload("share-1")

        val firstIv = first.copyOfRange(0, IV_SIZE)
        val secondIv = second.copyOfRange(0, IV_SIZE)
        assertNotEquals(Base64.encodeToString(firstIv, Base64.NO_WRAP), Base64.encodeToString(secondIv, Base64.NO_WRAP))
    }

    @Test
    fun theStoredFormIsNotThePlaintext() {
        store.set("share-1", "correct horse battery staple")
        val stored = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .getString("share-1", null)
        assertTrue(stored != null && !stored.contains("correct horse"))
    }

    /**
     * A tampered payload must not decrypt, and must fail as a [BridgeException].
     * An `AEADBadTagException` crossing the FFI has no mapping on the Rust side,
     * and an unmapped callback exception panics the calling thread (B18).
     */
    @Test
    fun aTamperedPayloadFailsToDecrypt() {
        store.set("share-1", "value")
        val payload = rawPayload("share-1")
        payload[payload.size - 1] = (payload[payload.size - 1].toInt() xor 0x01).toByte()
        writeRawPayload("share-1", payload)

        assertThrows(BridgeException::class.java) { store.get("share-1") }
    }

    @Test
    fun aTruncatedPayloadIsRejectedBeforeDecryption() {
        store.set("share-1", "value")
        writeRawPayload("share-1", rawPayload("share-1").copyOfRange(0, IV_SIZE))

        assertThrows(BridgeException::class.java) { store.get("share-1") }
    }

    /**
     * The device-side half of B18: this is what a lockscreen change or a device
     * restore does to the key. It must fail the read rather than the process,
     * and the store must still accept a re-entered secret afterwards — which is
     * what the shares screen's "Re-enter link" depends on.
     */
    @Test
    fun aLostKeyFailsTheReadRatherThanTheProcess() {
        store.set("share-1", "value")
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(KEY_ALIAS)

        assertThrows(BridgeException::class.java) { store.get("share-1") }

        // The store must still be usable for a re-entered secret.
        store.set("share-1", "re-entered")
        assertEquals("re-entered", store.get("share-1"))
    }

    private fun rawPayload(key: String): ByteArray {
        val encoded = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .getString(key, null) ?: error("no payload for $key")
        return Base64.decode(encoded, Base64.NO_WRAP)
    }

    private fun writeRawPayload(key: String, payload: ByteArray) {
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .edit()
            .putString(key, Base64.encodeToString(payload, Base64.NO_WRAP))
            .commit()
    }

    private fun clearPreferences() {
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE).edit().clear().commit()
    }

    private companion object {
        const val PREFERENCES = "proton_stream_secrets"
        const val KEY_ALIAS = "proton-stream-share-secrets-v1"
        const val IV_SIZE = 12
    }
}
