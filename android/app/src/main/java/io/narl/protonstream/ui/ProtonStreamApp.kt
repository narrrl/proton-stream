package io.narl.protonstream.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteScaffold
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.livedata.observeAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.work.WorkManager
import io.narl.protonstream.settings.SettingsStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import io.narl.protonstream.download.DownloadCoordinator
import io.narl.protonstream.download.DownloadStateStore
import io.narl.protonstream.download.RetainedDownload
import io.narl.protonstream.native.NativeRuntime
import uniffi.pstr_android.ShareRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TrackPreferencesRecord
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.MatchRecord
import uniffi.pstr_android.MetadataProvider
import io.narl.protonstream.playback.NativeMpvHost
import io.narl.protonstream.playback.LibmpvPlayerSurface

private enum class Destination(val label: String, val glyph: String) {
    Library("Library", "▦"),
    Shares("Shares", "⇄"),
    Downloads("Downloads", "⇩"),
    Settings("Settings", "⚙"),
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProtonStreamApp(
    playerHost: NativeMpvHost? = null,
    model: AppViewModel = viewModel(
        factory = AppViewModel.Factory(
            LocalContext.current,
            WorkManager.getInstance(LocalContext.current),
        ),
    ),
) {
    val state by model.state.collectAsState()
    val snackbars = remember { SnackbarHostState() }
    var destination by remember { mutableStateOf(Destination.Library) }
    var selectedTitle by remember { mutableStateOf<TitleRecord?>(null) }

    LaunchedEffect(state.titles, selectedTitle?.key) {
        selectedTitle?.let { selected ->
            selectedTitle = state.titles.firstOrNull { it.key == selected.key } ?: selected
        }
    }

    LaunchedEffect(state.message) {
        state.message?.let {
            snackbars.showSnackbar(it)
            model.dismissMessage()
        }
    }
    NavigationSuiteScaffold(
        navigationSuiteItems = {
            Destination.entries.forEach { item ->
                item(
                    selected = destination == item,
                    onClick = { destination = item; selectedTitle = null },
                    icon = { Text(item.glyph) },
                    label = { Text(item.label) },
                )
            }
        },
    ) {
        Scaffold(
            snackbarHost = { SnackbarHost(snackbars) },
            topBar = {
                TopAppBar(
                    title = { Text(selectedTitle?.name ?: "proton-stream") },
                    actions = {
                        if (destination == Destination.Library) {
                            FilledTonalButton(onClick = model::refresh, enabled = !state.refreshing) {
                                Text(if (state.refreshing) "Refreshing…" else "Refresh")
                            }
                            Spacer(Modifier.width(12.dp))
                        }
                    },
                )
            },
        ) { padding ->
            AnimatedContent(destination, label = "primary navigation") { target ->
                when (target) {
                    Destination.Library -> if (selectedTitle == null) {
                        LibraryScreen(state, model::search, { selectedTitle = it }, padding)
                    } else {
                        TitleScreen(
                            selectedTitle!!,
                            playerHost,
                            { selectedTitle = null },
                            model::reportError,
                            model::reloadAfterMetadataChange,
                            padding,
                        )
                    }
                    Destination.Shares -> SharesScreen(state.shares, model::addShare, model::removeShare, padding)
                    Destination.Downloads -> DownloadsScreen(
                        state,
                        model::removeOffline,
                        model::pauseDownload,
                        model::resumeDownload,
                        model::deletePartial,
                        padding,
                    )
                    Destination.Settings -> SettingsScreen(state, model::saveMetadataSettings, padding)
                }
            }
        }
    }

}

@Composable
private fun LibraryScreen(
    state: AppUiState,
    onSearch: (String) -> Unit,
    onTitle: (TitleRecord) -> Unit,
    padding: PaddingValues,
) {
    Column(Modifier.fillMaxSize().padding(padding).padding(horizontal = 16.dp)) {
        OutlinedTextField(
            value = state.query,
            onValueChange = onSearch,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            placeholder = { Text("Search library") },
        )
        Spacer(Modifier.height(16.dp))
        if (state.loading) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        } else if (state.titles.isEmpty()) {
            EmptyState("Your library is empty", "Add a Proton Drive public link under Shares, then refresh.")
        } else {
            LazyVerticalGrid(
                columns = GridCells.Adaptive(180.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
                contentPadding = PaddingValues(bottom = 24.dp),
            ) {
                items(state.titles, key = { it.key }) { title ->
                    Card(Modifier.fillMaxWidth().clickable { onTitle(title) }) {
                        RemoteArtwork(
                            title.backdropUrl ?: title.posterUrl,
                            title.canonicalName ?: title.name,
                            Modifier.fillMaxWidth().height(210.dp),
                        )
                        Column(Modifier.padding(12.dp)) {
                            Text(title.name, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            Text(
                                listOfNotNull(title.year?.toString(), "${title.episodeCount} episodes").joinToString(" · "),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun TitleScreen(
    title: TitleRecord,
    playerHost: NativeMpvHost?,
    onBack: () -> Unit,
    onPreferenceError: (Throwable) -> Unit,
    onMetadataChanged: () -> Unit,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var audioLanguage by remember(title.key) { mutableStateOf("") }
    var subtitleLanguage by remember(title.key) { mutableStateOf("") }
    var subtitlesEnabled by remember(title.key) { mutableStateOf(false) }
    var playing by remember(title.key) { mutableStateOf<EpisodeRecord?>(null) }
    var showMatch by remember(title.key) { mutableStateOf(false) }
    LaunchedEffect(title.key) {
        runCatching {
            withContext(Dispatchers.IO) {
                NativeRuntime.engine().titleTrackPreferences(title.key)
            }
        }.onSuccess { preferences ->
            audioLanguage = preferences.audioLanguage.orEmpty()
            subtitleLanguage = preferences.subtitleLanguage.orEmpty()
            subtitlesEnabled = preferences.subtitles
        }.onFailure(onPreferenceError)
    }
    if (playing != null) {
        Box(Modifier.fillMaxSize().padding(padding).background(Color.Black)) {
            LibmpvPlayerSurface(title.key, playing!!, playerHost)
            FilledTonalButton(onClick = { playerHost?.stop(); playing = null }, Modifier.padding(16.dp)) { Text("Back") }
        }
        return
    }
    LazyColumn(
        modifier = Modifier.fillMaxSize().padding(padding),
        contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        item {
            FilledTonalButton(onClick = onBack) { Text("Back to library") }
            RemoteArtwork(
                title.backdropUrl ?: title.posterUrl,
                title.canonicalName ?: title.name,
                Modifier.fillMaxWidth().aspectRatio(16f / 9f).padding(top = 12.dp),
            )
            Text(title.canonicalName ?: title.name, style = MaterialTheme.typography.headlineMedium, modifier = Modifier.padding(top = 12.dp))
            title.originalName?.takeIf { it != title.canonicalName }?.let {
                Text(it, style = MaterialTheme.typography.bodyMedium)
            }
            Text(
                listOfNotNull(
                    (title.metadataYear ?: title.year)?.toString(),
                    title.rating?.let { "%.1f/10".format(it) },
                    title.genres.takeIf { it.isNotEmpty() }?.joinToString(" · "),
                ).joinToString("  •  "),
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(top = 6.dp),
            )
            title.overview?.let { Text(it, modifier = Modifier.padding(top = 10.dp)) }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 12.dp)) {
                Button(onClick = { DownloadCoordinator.enqueue(context, title.seasons.flatMap { it.episodes }) }) {
                    Text("Download show")
                }
                FilledTonalButton(onClick = { showMatch = true }) { Text("Change match") }
            }
            Text("${title.watchedCount} of ${title.episodeCount} watched", Modifier.padding(vertical = 12.dp))
            Text("Preferred tracks", style = MaterialTheme.typography.titleMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    audioLanguage,
                    { audioLanguage = it },
                    label = { Text("Audio language") },
                    modifier = Modifier.weight(1f),
                    singleLine = true,
                )
                OutlinedTextField(
                    subtitleLanguage,
                    { subtitleLanguage = it },
                    label = { Text("Subtitle language") },
                    modifier = Modifier.weight(1f),
                    singleLine = true,
                )
            }
            SettingToggle("Enable subtitles", subtitlesEnabled) { subtitlesEnabled = it }
            FilledTonalButton(onClick = {
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            NativeRuntime.engine().setTitleTrackPreferences(
                                title.key,
                                TrackPreferencesRecord(
                                    audioLanguage.takeIf(String::isNotBlank),
                                    subtitleLanguage.takeIf(String::isNotBlank),
                                    subtitlesEnabled,
                                ),
                            )
                        }
                    }.onFailure(onPreferenceError)
                }
            }) { Text("Save track preferences") }
        }
        title.seasons.forEach { season ->
            item {
                Row(
                    Modifier.fillMaxWidth().padding(top = 12.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(season.label, style = MaterialTheme.typography.titleLarge)
                    FilledTonalButton(onClick = { DownloadCoordinator.enqueue(context, season.episodes) }) {
                        Text("Download season")
                    }
                }
            }
            items(season.episodes, key = { it.linkId }) { episode ->
                Card(Modifier.fillMaxWidth()) {
                    Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Button(onClick = { playing = episode }, enabled = playerHost != null) {
                            Text(if (playerHost == null) "Player loading…" else "Play")
                        }
                        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                            Text(episode.label, fontWeight = FontWeight.SemiBold)
                            Text(episode.detail, style = MaterialTheme.typography.bodySmall, maxLines = 1)
                        }
                        FilledTonalButton(
                            onClick = { DownloadCoordinator.enqueue(context, episode) },
                            enabled = !episode.offline,
                        ) { Text(if (episode.offline) "Offline" else "Download") }
                    }
                }
            }
        }
    }
    if (showMatch) {
        ChangeMatchDialog(
            title = title,
            onDismiss = { showMatch = false },
            onChanged = { showMatch = false; onMetadataChanged() },
            onError = onPreferenceError,
        )
    }
}

@Composable
private fun ChangeMatchDialog(
    title: TitleRecord,
    onDismiss: () -> Unit,
    onChanged: () -> Unit,
    onError: (Throwable) -> Unit,
) {
    var term by remember(title.key) { mutableStateOf(title.canonicalName ?: title.name) }
    var searching by remember { mutableStateOf(false) }
    var options by remember { mutableStateOf<List<MatchRecord>>(emptyList()) }
    val scope = rememberCoroutineScope()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Change match") },
        text = {
            Column {
                Text("Search the configured metadata provider. Nothing is stored until you choose an entry.")
                OutlinedTextField(term, { term = it }, Modifier.fillMaxWidth().padding(vertical = 8.dp), singleLine = true)
                Button(enabled = term.isNotBlank() && !searching, onClick = {
                    searching = true
                    scope.launch {
                        runCatching {
                            withContext(Dispatchers.IO) { NativeRuntime.engine().searchMatches(title.key, term) }
                        }.onSuccess { options = it }.onFailure(onError)
                        searching = false
                    }
                }) { Text(if (searching) "Searching…" else "Search") }
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 300.dp)) {
                    items(options, key = { "${it.provider}:${it.remoteId}" }) { option ->
                        OutlinedButton(
                            onClick = {
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.IO) { NativeRuntime.engine().chooseMatch(title.key, option) }
                                    }.onSuccess { onChanged() }.onFailure(onError)
                                }
                            },
                            modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
                        ) {
                            Column(Modifier.fillMaxWidth()) {
                                Text(option.name, fontWeight = FontWeight.SemiBold)
                                Text(listOfNotNull(option.year?.toString(), option.originalName).joinToString(" · "))
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                scope.launch {
                    runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().forgetMatch(title.key) } }
                        .onSuccess { onChanged() }.onFailure(onError)
                }
            }) { Text("Forget match") }
        },
        dismissButton = { FilledTonalButton(onClick = onDismiss) { Text("Close") } },
    )
}

@Composable
private fun SharesScreen(
    shares: List<ShareRecord>,
    onAdd: (String, String, String?) -> Unit,
    onRemove: (String) -> Unit,
    padding: PaddingValues,
) {
    var showAdd by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().padding(padding).padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("Proton Drive public links", style = MaterialTheme.typography.titleLarge)
            Button(onClick = { showAdd = true }) { Text("Add share") }
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 16.dp)) {
            items(shares, key = { it.id }) { share ->
                Card(Modifier.fillMaxWidth()) {
                    Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(share.name, fontWeight = FontWeight.SemiBold)
                            Text(if (share.hasCustomPassword) "Custom password stored securely" else "Public link")
                        }
                        FilledTonalButton(onClick = { onRemove(share.id) }) { Text("Remove") }
                    }
                }
            }
        }
    }
    if (showAdd) AddShareDialog(onDismiss = { showAdd = false }, onAdd = onAdd)
}

@Composable
private fun AddShareDialog(onDismiss: () -> Unit, onAdd: (String, String, String?) -> Unit) {
    var name by remember { mutableStateOf("") }
    var url by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Add Proton Drive share") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(name, { name = it }, label = { Text("Library name") }, singleLine = true)
                OutlinedTextField(
                    url,
                    { url = it },
                    label = { Text("Public share URL") },
                    singleLine = true,
                    // The URL fragment is a secret. Password input disables
                    // IME learning/suggestions while normal long-press paste
                    // remains available.
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
                OutlinedTextField(
                    password,
                    { password = it },
                    label = { Text("Custom password (optional)") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
            }
        },
        confirmButton = {
            Button(
                onClick = { onAdd(name.trim(), url.trim(), password); onDismiss() },
                enabled = name.isNotBlank() && url.isNotBlank(),
            ) { Text("Add") }
        },
        dismissButton = { FilledTonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun DownloadsScreen(
    state: AppUiState,
    onRemove: (uniffi.pstr_android.OfflineRecord) -> Unit,
    onPause: (RetainedDownload) -> Unit,
    onResume: (RetainedDownload) -> Unit,
    onDeletePartial: (RetainedDownload) -> Unit,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    val work = WorkManager.getInstance(context).getWorkInfosByTagLiveData(DownloadCoordinator.TAG)
    val downloads by work.observeAsState(emptyList())
    // Reading retained metadata on every WorkInfo transition also hydrates
    // paused/failed/cancelled entries after WorkManager history is pruned.
    val retained = remember(downloads) { DownloadStateStore(context).records() }
    Column(Modifier.fillMaxSize().padding(padding).padding(16.dp)) {
        Text("Offline downloads", style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.height(16.dp))
        if (retained.isEmpty() && state.offline.isEmpty()) {
            EmptyState("No downloads", "Episodes, seasons, and shows saved offline appear here.")
        }
        state.offline.forEach { file ->
            val episode = file.episode
            Card(Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
                Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(episode?.label ?: file.linkId, fontWeight = FontWeight.SemiBold)
                        Text(formatBytes(file.size))
                    }
                    FilledTonalButton(onClick = { onRemove(file) }) { Text("Make online only") }
                }
            }
        }
        retained.forEach { download ->
            val progress = if (download.total > 0L) download.downloaded.toFloat() / download.total else 0f
            Card(Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
                Column(Modifier.padding(16.dp)) {
                    Text(download.label, fontWeight = FontWeight.SemiBold)
                    Text(download.status.replaceFirstChar { it.uppercase() })
                    if (download.total > 0L) {
                        LinearProgressIndicator(
                            progress = { progress },
                            modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                        )
                        Text("${formatBytes(download.downloaded.toULong())} of ${formatBytes(download.total.toULong())}", style = MaterialTheme.typography.bodySmall)
                    }
                    download.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                    Row(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        modifier = Modifier.padding(top = 8.dp),
                    ) {
                        if (download.status == RetainedDownload.STATUS_RUNNING ||
                            download.status == RetainedDownload.STATUS_QUEUED
                        ) {
                            FilledTonalButton(onClick = { onPause(download) }) { Text("Pause") }
                        } else {
                            FilledTonalButton(onClick = { onResume(download) }) { Text("Resume") }
                        }
                        TextButton(onClick = { onDeletePartial(download) }) { Text("Delete partial") }
                    }
                }
            }
        }
    }
}

@Composable
private fun SettingsScreen(
    state: AppUiState,
    onSaveMetadata: (Boolean, MetadataProvider, String, String) -> Unit,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    val settings = remember { SettingsStore(context) }
    var wifiOnly by remember { mutableStateOf(settings.wifiOnly) }
    var backgroundAudio by remember { mutableStateOf(settings.backgroundAudio) }
    var showMetadata by remember { mutableStateOf(false) }
    var legalDocument by remember { mutableStateOf<LegalDocument?>(null) }
    Column(Modifier.fillMaxSize().padding(padding).padding(16.dp)) {
        Text("Settings", style = MaterialTheme.typography.titleLarge)
        SettingToggle("Download on Wi-Fi only", wifiOnly) { wifiOnly = it; settings.wifiOnly = it }
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
        Text("Metadata enrichment", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        Text(
            if (state.metadataSettings.enabled) "On · ${state.metadataSettings.provider.displayName()}"
            else "Off (privacy default)",
        )
        Text(
            "Enabling this sends library title names to the selected third-party provider over HTTPS.",
            style = MaterialTheme.typography.bodySmall,
        )
        FilledTonalButton(onClick = { showMetadata = true }, modifier = Modifier.padding(vertical = 10.dp)) {
            Text("Configure metadata")
        }
        HorizontalDivider()
        Text("Storage", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 20.dp))
        Text("Offline media is encrypted at rest by Android and kept in app-private storage.")
        Text("proton-stream Android · GPL-3.0-or-later", style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(top = 24.dp))
        Text(
            "This program comes with absolutely no warranty. You may redistribute it under the GNU GPL.",
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(top = 8.dp),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 12.dp)) {
            FilledTonalButton(onClick = {
                legalDocument = LegalDocument("GNU GPL v3", "licenses/GPL-3.0.txt")
            }) { Text("View license") }
            FilledTonalButton(onClick = {
                legalDocument = LegalDocument("Third-party notices", "licenses/THIRD_PARTY_NOTICES.md")
            }) { Text("View notices") }
        }
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
                        if (option == provider) Button(onClick = { provider = option }) { Text(option.displayName()) }
                        else OutlinedButton(onClick = { provider = option }) { Text(option.displayName()) }
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
            Button(
                enabled = !enabled || provider != MetadataProvider.TMDB || current.ready || apiKey.isNotBlank(),
                onClick = { onSave(enabled, provider, language.ifBlank { "en" }, apiKey) },
            ) { Text("Save") }
        },
        dismissButton = { FilledTonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

private fun MetadataProvider.displayName(): String = when (this) {
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
        confirmButton = { Button(onClick = onDismiss) { Text("Close") } },
    )
}

@Composable
private fun SettingToggle(label: String, checked: Boolean, onChecked: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChecked)
    }
}

@Composable
private fun EmptyState(title: String, body: String) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(body, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

private fun formatBytes(bytes: ULong): String {
    val value = bytes.toDouble()
    return when {
        value >= 1024 * 1024 * 1024 -> "%.1f GiB".format(value / (1024 * 1024 * 1024))
        value >= 1024 * 1024 -> "%.1f MiB".format(value / (1024 * 1024))
        else -> "${bytes} bytes"
    }
}
