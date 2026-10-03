package io.narl.protonstream.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.border
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import androidx.compose.ui.unit.dp
import io.narl.protonstream.settings.SettingsStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import io.narl.protonstream.download.DownloadCoordinator
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.AppearanceState
import io.narl.protonstream.ui.theme.EdgedButton
import io.narl.protonstream.ui.theme.TonalButton
import uniffi.pstr_android.MetadataProvider
import uniffi.pstr_android.FlavorChoice
import uniffi.pstr_android.AppearanceRecord
import uniffi.pstr_android.AccentChoice
import uniffi.pstr_android.PlaybackPrefsRecord

@Composable
internal fun SettingsScreen(
    state: AppUiState,
    onSaveMetadata: (Boolean, MetadataProvider, String, String) -> Unit,
    onMatchAgain: () -> Unit,
    onClearCache: () -> Unit,
    onRemoveAllOffline: () -> Unit,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    val settings = remember { SettingsStore(context) }
    var wifiOnly by remember { mutableStateOf(settings.wifiOnly) }
    var backgroundAudio by remember { mutableStateOf(settings.backgroundAudio) }
    var hardwareDecoding by remember { mutableStateOf(settings.hardwareDecoding) }
    var cacheBudgetGib by remember { mutableStateOf(settings.cacheBudgetGib) }
    // Playback preferences are the shared store's, not this app's: they are the
    // same file the desktop client reads, so a language chosen on one is the
    // language the other starts in.
    var prefs by remember { mutableStateOf<PlaybackPrefsRecord?>(null) }
    val scope = rememberCoroutineScope()
    fun update(change: (PlaybackPrefsRecord) -> PlaybackPrefsRecord) {
        val next = change(prefs ?: return)
        prefs = next
        scope.launch(Dispatchers.IO) {
            runCatching { NativeRuntime.engine().setPlaybackPrefs(next) }
        }
    }
    LaunchedEffect(Unit) {
        runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().playbackPrefs() } }
            .onSuccess { prefs = it }
    }
    var confirmDelete by remember { mutableStateOf(false) }
    var showMetadata by remember { mutableStateOf(false) }
    var legalDocument by remember { mutableStateOf<LegalDocument?>(null) }
    Column(Modifier.fillMaxSize().padding(padding).padding(16.dp).verticalScroll(rememberScrollState())) {
        Text("Settings", style = MaterialTheme.typography.titleLarge)
        SettingToggle("Download on Wi-Fi only", wifiOnly) {
            wifiOnly = it
            settings.wifiOnly = it
            // Constraints are baked in at enqueue time, so a queue that already
            // exists keeps the policy it was queued under until it is re-issued.
            DownloadCoordinator.applyNetworkPolicy(context)
        }
        HorizontalDivider()
        SettingToggle("Continue audio in the background", backgroundAudio) {
            backgroundAudio = it
            settings.backgroundAudio = it
        }
        Text(
            "When disabled, leaving playback stops the player instead of keeping a media notification active.",
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(bottom = 14.dp),
        )
        HorizontalDivider()
        Text("Playback", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        SettingToggle("Hardware decoding", hardwareDecoding) {
            hardwareDecoding = it
            settings.hardwareDecoding = it
        }
        Text(
            "Turn off if video shows green or torn frames. Decoding in software uses more " +
                "battery. Applies from the next episode.",
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(bottom = 14.dp),
        )
        prefs?.let { current ->
            SettingToggle("Play the next episode automatically", current.autoplayNext) { on ->
                update { it.copy(autoplayNext = on) }
            }
            SettingToggle("Skip openings and endings automatically", current.autoSkip) { on ->
                update { it.copy(autoSkip = on) }
            }
            Text(
                "Openings and endings are read from the chapters a release was muxed with. " +
                    "With this off you get a Skip button instead, which is the safer default: " +
                    "a mis-named chapter then costs a tap rather than a scene.",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(bottom = 14.dp),
            )
            SettingToggle("Show subtitles", current.subtitles) { on ->
                update { it.copy(subtitles = on) }
            }
            // Language tags, not a picker: which languages exist is a property
            // of each file, and a list built from one episode is wrong for the
            // next. A show that has been given its own choice keeps it.
            LanguageField("Preferred audio language", current.audioLanguage) { tag ->
                update { it.copy(audioLanguage = tag) }
            }
            LanguageField("Preferred subtitle language", current.subtitleLanguage) { tag ->
                update { it.copy(subtitleLanguage = tag) }
            }
            Text(
                "Three-letter tags as they appear in the file — \"jpn\", \"eng\". Used for every " +
                    "title that has not been given a track choice of its own.",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(bottom = 14.dp),
            )
        }
        HorizontalDivider()
        Text("Appearance", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        AppearancePicker()
        HorizontalDivider()
        Text("Metadata enrichment", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        Text(
            if (state.metadataSettings.enabled) "On · ${state.metadataSettings.provider.displayName()}"
            else "Off (privacy default)",
        )
        Text(
            "Enabling this sends library title names to the selected third-party provider over HTTPS.",
            style = MaterialTheme.typography.bodySmall,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(vertical = 10.dp)) {
            TonalButton(onClick = { showMetadata = true }) { Text("Configure metadata") }
            // Every title looked up again, matched ones included: the way out of
            // a library the provider answered wrong, which otherwise stays wrong
            // for as long as the match is remembered.
            TonalButton(
                onClick = { onMatchAgain() },
                enabled = state.metadataSettings.enabled && !state.refreshing,
            ) { Text("Match everything again") }
        }
        HorizontalDivider()
        Text("Storage", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        Text("Offline media is encrypted at rest by Android and kept in app-private storage.")
        Text(
            "${state.storage.offlineCount} episodes offline · ${formatBytes(state.storage.offlineBytes)}",
            modifier = Modifier.padding(top = 8.dp),
        )
        if (state.storage.partialBytes > 0uL) {
            Text(
                "Unfinished downloads · ${formatBytes(state.storage.partialBytes)}",
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Text(
            "Streaming cache · ${formatBytes(state.storage.cacheBytes)} of $cacheBudgetGib GiB",
            style = MaterialTheme.typography.bodySmall,
        )
        // What has been watched is kept up to this much, so going back over a
        // scene costs no download. A lower choice frees the difference at once.
        LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 8.dp)) {
            items(SettingsStore.CACHE_BUDGET_CHOICES, key = { it }) { gib ->
                if (gib == cacheBudgetGib) {
                    AccentButton(onClick = {}) { Text("$gib GiB") }
                } else {
                    EdgedButton(onClick = {
                        cacheBudgetGib = gib
                        settings.cacheBudgetGib = gib
                        scope.launch(Dispatchers.IO) {
                            runCatching {
                                NativeRuntime.engine().setStreamCacheBudget(SettingsStore.gibToBytes(gib))
                            }
                        }
                    }) { Text("$gib GiB") }
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 10.dp)) {
            // The cache is rebuildable, so it goes without asking. Offline
            // episodes are a choice the viewer made, so that one asks.
            TonalButton(onClick = onClearCache) { Text("Clear cache") }
            TonalButton(
                onClick = { confirmDelete = true },
                enabled = state.storage.offlineCount > 0uL,
            ) { Text("Delete all offline") }
        }
        Text("proton-stream Android · GPL-3.0-or-later", style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(top = 24.dp))
        Text(
            "This program comes with absolutely no warranty. You may redistribute it under the GNU GPL.",
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(top = 8.dp),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 12.dp)) {
            TonalButton(onClick = {
                legalDocument = LegalDocument("GNU GPL v3", "licenses/GPL-3.0.txt")
            }) { Text("View license") }
            TonalButton(onClick = {
                legalDocument = LegalDocument("Third-party notices", "licenses/THIRD_PARTY_NOTICES.md")
            }) { Text("View notices") }
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("Delete all offline episodes?") },
            text = {
                Text(
                    "${state.storage.offlineCount} episodes (${formatBytes(state.storage.offlineBytes)}) " +
                        "will be removed from this device. Watch history is kept, and anything " +
                        "deleted can be downloaded again.",
                )
            },
            confirmButton = {
                AccentButton(onClick = { confirmDelete = false; onRemoveAllOffline() }) { Text("Delete") }
            },
            dismissButton = { TonalButton(onClick = { confirmDelete = false }) { Text("Cancel") } },
        )
    }
    legalDocument?.let { document ->
        LegalDocumentDialog(document, onDismiss = { legalDocument = null })
    }
    if (showMetadata) {
        MetadataSettingsDialog(
            current = state.metadataSettings,
            onDismiss = { showMetadata = false },
            onSave = { enabled, provider, language, key ->
                onSaveMetadata(enabled, provider, language, key)
                showMetadata = false
            },
        )
    }
}

@Composable
private fun MetadataSettingsDialog(
    current: uniffi.pstr_android.MetadataSettingsRecord,
    onDismiss: () -> Unit,
    onSave: (Boolean, MetadataProvider, String, String) -> Unit,
) {
    var enabled by remember { mutableStateOf(current.enabled) }
    var provider by remember { mutableStateOf(current.provider) }
    var language by remember { mutableStateOf(current.language) }
    var apiKey by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Metadata enrichment") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Off by default: enabling sends the titles in your library to a third party, associated with your IP address and subject to their privacy policy.")
                SettingToggle("Enable enrichment", enabled) { enabled = it }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    MetadataProvider.entries.forEach { option ->
                        if (option == provider) AccentButton(onClick = { provider = option }) { Text(option.displayName()) }
                        else EdgedButton(onClick = { provider = option }) { Text(option.displayName()) }
                    }
                }
                Text(
                    if (provider == MetadataProvider.ANI_LIST) "Anime; no account or API key required."
                    else "Film and television; requires a free TMDB API key.",
                    style = MaterialTheme.typography.bodySmall,
                )
                if (provider == MetadataProvider.TMDB) {
                    OutlinedTextField(language, { language = it }, label = { Text("Language") }, singleLine = true)
                    OutlinedTextField(
                        apiKey,
                        { apiKey = it },
                        label = { Text(if (current.ready) "TMDB API key (leave blank to keep)" else "TMDB API key") },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        singleLine = true,
                    )
                }
            }
        },
        confirmButton = {
            AccentButton(
                enabled = !enabled || provider != MetadataProvider.TMDB || current.ready || apiKey.isNotBlank(),
                onClick = { onSave(enabled, provider, language.ifBlank { "en" }, apiKey) },
            ) { Text("Save") }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

internal fun MetadataProvider.displayName(): String = when (this) {
    MetadataProvider.ANI_LIST -> "AniList"
    MetadataProvider.TMDB -> "TMDB"
}

private data class LegalDocument(val title: String, val assetPath: String)

@Composable
private fun LegalDocumentDialog(document: LegalDocument, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var contents by remember(document.assetPath) { mutableStateOf("Loading…") }
    LaunchedEffect(document.assetPath) {
        contents = runCatching {
            withContext(Dispatchers.IO) {
                context.assets.open(document.assetPath).bufferedReader().use { it.readText() }
            }
        }.getOrElse { error -> "Unable to load this document: ${error.message}" }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(document.title) },
        text = {
            LazyColumn(Modifier.fillMaxWidth().heightIn(max = 520.dp)) {
                item { Text(contents, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = { AccentButton(onClick = onDismiss) { Text("Close") } },
    )
}

@Composable
internal fun SettingToggle(label: String, checked: Boolean, onChecked: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChecked)
    }
}

/**
 * Flavour, accent and gradients — the same three the desktop client offers.
 *
 * Every colour is resolved by Rust, so a swatch here is the colour the app will
 * actually paint rather than an approximation of it, and the choice is stored in
 * the file both clients read.
 */
@Composable
private fun AppearancePicker() {
    val scope = rememberCoroutineScope()
    var choice by remember { mutableStateOf<AppearanceRecord?>(null) }
    var swatches by remember { mutableStateOf<Map<AccentChoice, Color>>(emptyMap()) }

    suspend fun repaint(next: AppearanceRecord, store: Boolean) {
        runCatching {
            withContext(Dispatchers.IO) {
                val engine = NativeRuntime.engine()
                if (store) engine.setAppearance(next)
                val palette = engine.previewPalette(next)
                // Every accent as it would look in *this* flavour: a swatch row
                // that keeps Mocha's pastels while Latte is selected is a row
                // that lies about what the next tap does.
                val row = AccentChoice.entries.associateWith { accent ->
                    Color(engine.previewPalette(next.copy(accent = accent)).accent.toInt())
                }
                palette to row
            }
        }.onSuccess { (palette, row) ->
            choice = next
            swatches = row
            AppearanceState.apply(palette)
        }
    }

    LaunchedEffect(Unit) {
        runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().appearance() } }
            .onSuccess { repaint(it, store = false) }
    }

    val current = choice ?: return
    Text("Palette", style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(top = 8.dp))
    LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(vertical = 8.dp)) {
        items(FlavorChoice.entries.toList(), key = { it.name }) { flavor ->
            val selected = flavor == current.flavor
            if (selected) {
                AccentButton(onClick = {}) { Text(flavor.label()) }
            } else {
                EdgedButton(onClick = {
                    scope.launch { repaint(current.copy(flavor = flavor), store = true) }
                }) { Text(flavor.label()) }
            }
        }
    }
    Text("Accent", style = MaterialTheme.typography.bodyMedium)
    LazyRow(horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.padding(vertical = 8.dp)) {
        items(AccentChoice.entries.toList(), key = { it.name }) { accent ->
            val swatch = swatches[accent] ?: MaterialTheme.colorScheme.surfaceVariant
            Box(
                Modifier
                    .size(36.dp)
                    .clip(CircleShape)
                    .background(swatch)
                    .border(
                        width = if (accent == current.accent) 3.dp else 1.dp,
                        color = if (accent == current.accent) {
                            MaterialTheme.colorScheme.onBackground
                        } else {
                            MaterialTheme.colorScheme.outline
                        },
                        shape = CircleShape,
                    )
                    .clickable {
                        scope.launch { repaint(current.copy(accent = accent), store = true) }
                    },
            )
        }
    }
    SettingToggle("Paint the accent as a gradient", current.gradients) { on ->
        scope.launch { repaint(current.copy(gradients = on), store = true) }
    }
    Text(
        "Off is the safer setting on a panel that bands: a slow ramp across a wide bar " +
            "shows every step it is drawn from, and flat is better than striped.",
        style = MaterialTheme.typography.bodySmall,
        modifier = Modifier.padding(bottom = 14.dp),
    )
}

/** What each palette family is called. The desktop client says the same. */
private fun FlavorChoice.label() = when (this) {
    FlavorChoice.PROTON -> "Proton"
    FlavorChoice.LATTE -> "Catppuccin Latte"
    FlavorChoice.FRAPPE -> "Catppuccin Frappé"
    FlavorChoice.MACCHIATO -> "Catppuccin Macchiato"
    FlavorChoice.MOCHA -> "Catppuccin Mocha"
    FlavorChoice.PERSONA5 -> "Persona 5"
}

/**
 * One language tag, committed as it is typed.
 *
 * Blank is a real answer and means "no preference" — the bridge stores it as
 * absent, which is what leaves the choice to the container's own default track.
 */
@Composable
private fun LanguageField(label: String, value: String?, onChange: (String?) -> Unit) {
    OutlinedTextField(
        value = value.orEmpty(),
        onValueChange = { onChange(it.trim().takeIf(String::isNotEmpty)) },
        label = { Text(label) },
        singleLine = true,
        modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
    )
}
