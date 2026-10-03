package io.narl.protonstream.settings

import android.content.Context

/**
 * The settings that are Android's alone.
 *
 * Playback preferences — volume, languages, autoplay, auto-skip — deliberately
 * do *not* live here: they are `pstr_core::prefs`, reached over the bridge, so
 * there is one definition of them rather than one per front end.
 */
class SettingsStore(context: Context) {
    private val preferences = context.getSharedPreferences(NAME, Context.MODE_PRIVATE)

    var wifiOnly: Boolean
        get() = preferences.getBoolean(KEY_WIFI_ONLY, true)
        set(value) { preferences.edit().putBoolean(KEY_WIFI_ONLY, value).apply() }

    var backgroundAudio: Boolean
        get() = preferences.getBoolean(KEY_BACKGROUND_AUDIO, true)
        set(value) { preferences.edit().putBoolean(KEY_BACKGROUND_AUDIO, value).apply() }

    /**
     * Whether mpv may decode in hardware. Off is the way out for a device whose
     * MediaCodec decoder shows green or torn frames; it costs battery, so it is
     * on unless the viewer turns it off. Read at each load, so a change applies
     * from the next file.
     */
    var hardwareDecoding: Boolean
        get() = preferences.getBoolean(KEY_HARDWARE_DECODING, true)
        set(value) { preferences.edit().putBoolean(KEY_HARDWARE_DECODING, value).apply() }

    /** The streaming cache's budget, in GiB. See [CACHE_BUDGET_CHOICES]. */
    var cacheBudgetGib: Int
        get() = preferences.getInt(KEY_CACHE_BUDGET_GIB, DEFAULT_CACHE_BUDGET_GIB)
        set(value) { preferences.edit().putInt(KEY_CACHE_BUDGET_GIB, value).apply() }

    companion object {
        /** The desktop's range, in the steps a phone's storage is bought in. */
        val CACHE_BUDGET_CHOICES = listOf(1, 2, 4, 8, 16)

        /** `DiskCacheConfig::DEFAULT_BUDGET_BYTES`, so an unset choice changes nothing. */
        const val DEFAULT_CACHE_BUDGET_GIB = 4

        fun gibToBytes(gib: Int): ULong = gib.toULong() * 1024uL * 1024uL * 1024uL

        private const val NAME = "proton_stream_settings"
        private const val KEY_WIFI_ONLY = "wifi_only"
        private const val KEY_BACKGROUND_AUDIO = "background_audio"
        private const val KEY_HARDWARE_DECODING = "hardware_decoding"
        private const val KEY_CACHE_BUDGET_GIB = "cache_budget_gib"
    }
}
