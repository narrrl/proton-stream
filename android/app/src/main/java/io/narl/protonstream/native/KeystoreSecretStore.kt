package io.narl.protonstream.native

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import uniffi.pstr_android.AndroidSecretStore
import uniffi.pstr_android.BridgeException

/**
 * Encrypts share fragments and passwords before they reach app preferences.
 *
 * Every failure here leaves as a [BridgeException]. Raw Java exceptions crossing
 * the FFI have no mapping on the Rust side, and an unmapped callback exception
 * is a panic on whichever thread was calling — including a tokio worker holding
 * a download. What the caller does about it is a separate question; being told
 * at all is this class's part.
 */
class KeystoreSecretStore(context: Context) : AndroidSecretStore {
    private val preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
    private val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    override fun set(key: String, value: String) = bridging("store credential") {
        val payload = try {
            seal(value)
        } catch (invalidated: KeyPermanentlyInvalidatedException) {
            // A lockscreen change or a device restore retires the key. Nothing
            // it sealed can be read again — and nothing new can be sealed with
            // it either, so re-entering a link would fail here rather than
            // restoring access. Everything the old key held is already lost, so
            // replacing it costs nothing that is not gone.
            keyStore.deleteEntry(KEY_ALIAS)
            seal(value)
        }
        check(preferences.edit().putString(key, Base64.encodeToString(payload, Base64.NO_WRAP)).commit())
    }

    private fun seal(value: String): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, encryptionKey())
        return cipher.iv + cipher.doFinal(value.toByteArray(Charsets.UTF_8))
    }

    override fun get(key: String): String? = bridging("read credential") {
        val encoded = preferences.getString(key, null) ?: return@bridging null
        val payload = Base64.decode(encoded, Base64.NO_WRAP)
        require(payload.size > IV_SIZE) { "Invalid encrypted credential" }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(
            Cipher.DECRYPT_MODE,
            encryptionKey(),
            GCMParameterSpec(TAG_BITS, payload.copyOfRange(0, IV_SIZE)),
        )
        cipher.doFinal(payload.copyOfRange(IV_SIZE, payload.size)).toString(Charsets.UTF_8)
    }

    override fun delete(key: String) = bridging("delete credential") {
        check(preferences.edit().remove(key).commit())
    }

    private fun <T> bridging(action: String, block: () -> T): T = try {
        block()
    } catch (error: BridgeException) {
        throw error
    } catch (error: Exception) {
        // The message, never the stack trace: this reaches a snackbar, and the
        // trace is both unreadable there and a description of app internals.
        throw BridgeException.Failure("Could not $action: ${error.message ?: error::class.java.simpleName}")
    }

    private fun encryptionKey(): SecretKey {
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(
                KeyGenParameterSpec.Builder(
                    KEY_ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
            generateKey()
        }
    }

    private companion object {
        const val PREFERENCES = "proton_stream_secrets"
        const val KEY_ALIAS = "proton-stream-share-secrets-v1"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_SIZE = 12
        const val TAG_BITS = 128
    }
}
