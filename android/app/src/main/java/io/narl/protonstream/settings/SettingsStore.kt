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

    private companion object {
        const val NAME = "proton_stream_settings"
        const val KEY_WIFI_ONLY = "wifi_only"
        const val KEY_BACKGROUND_AUDIO = "background_audio"
    }
}
