package io.narl.protonstream.ui

import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.livedata.observeAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.work.WorkManager
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import io.narl.protonstream.download.DownloadCoordinator
import io.narl.protonstream.download.DownloadStateStore
import io.narl.protonstream.download.RetainedDownload
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.AccentProgress
import io.narl.protonstream.ui.theme.EdgedButton
import io.narl.protonstream.ui.theme.QuietButton
import io.narl.protonstream.ui.theme.TonalButton
import io.narl.protonstream.ui.theme.solid
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.TrackPreferencesRecord
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.MatchRecord

@Composable
internal fun TitleScreen(
    title: TitleRecord,
    playerReady: Boolean,
    onPlay: (List<EpisodeRecord>, Int) -> Unit,
    onBack: () -> Unit,
    onPreferenceError: (Throwable) -> Unit,
    onMetadataChanged: () -> Unit,
    onSetWatched: (EpisodeRecord, Boolean) -> Unit,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var audioLanguage by remember(title.key) { mutableStateOf("") }
    var subtitleLanguage by remember(title.key) { mutableStateOf("") }
    var subtitlesEnabled by remember(title.key) { mutableStateOf(false) }
    var showMatch by remember(title.key) { mutableStateOf(false) }
    var expandedSeasons by remember(title.key) { mutableStateOf(setOf(title.seasons.firstOrNull()?.label)) }

    BackHandler(onBack = onBack)

    LaunchedEffect(title.key) {
        runCatching {
            withContext(Dispatchers.IO) {
                NativeRuntime.engine().titleTrackPreferences(title.key)
            }
        }.onSuccess { preferences ->
            // Absent means this title has never been given a choice; the fields
            // are then empty and the global preference is what plays.
            audioLanguage = preferences?.audioLanguage.orEmpty()
            subtitleLanguage = preferences?.subtitleLanguage.orEmpty()
            subtitlesEnabled = preferences?.subtitles ?: true
        }.onFailure(onPreferenceError)
    }
    // Display order across seasons: what previous/next and autoplay walk, and
    // the same order the desktop client uses.
    val playlist = remember(title) { title.seasons.flatMap(SeasonRecord::episodes) }
    // Where a press on Play lands. Part-watched wins over unwatched, most
    // recently played first, which is the order the desktop client resumes in.
    val nextUp = remember(playlist) {
        val resumable = playlist.withIndex().filter { it.value.resumeAt != null }
        resumable.maxByOrNull { it.value.lastPlayed }?.index
            ?: playlist.indexOfFirst { !it.watched }.takeIf { it >= 0 }
            ?: 0
    }
    // Live download state per episode, so a row can show progress and be paused
    // without a trip to the Downloads tab.
    val work by WorkManager.getInstance(context)
        .getWorkInfosByTagLiveData(DownloadCoordinator.TAG).observeAsState(emptyList())
    val downloads = remember(work) { DownloadStateStore(context).records() }

    LazyColumn(
        modifier = Modifier.fillMaxSize().padding(padding),
        contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        item {
            QuietButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = null)
                Spacer(Modifier.width(8.dp))
                Text("Back to library")
            }
            RemoteArtwork(
                title.backdropUrl ?: title.posterUrl,
                title.canonicalName ?: title.name,
                Modifier.fillMaxWidth().aspectRatio(16f / 9f).padding(top = 12.dp),
                fallback = title.thumbnailSource,
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
            // Wrapping, not squeezing: a fixed row hands the last button the
            // width the others left over, which is how "More on AniList" ended
            // up set one letter per line on a phone.
            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
            ) {
                // What a press on the poster should play: whatever was left
                // part-watched, else the first unwatched episode, else the
                // first — the desktop client's `next_up`.
                AccentButton(onClick = { onPlay(playlist, nextUp) }, enabled = playerReady && playlist.isNotEmpty()) {
                    Text(
                        when {
                            !playerReady -> "Player loading…"
                            playlist.getOrNull(nextUp)?.resumeAt != null -> "Resume"
                            title.watchedCount > 0uL -> "Continue"
                            else -> "Play"
                        },
                    )
                }
                // Only where it would do something different: an unstarted show
                // already starts at the beginning.
                if (playlist.getOrNull(nextUp)?.resumeAt != null || title.watchedCount > 0uL) {
                    TonalButton(
                        onClick = { onPlay(playlist, 0) },
                        enabled = playerReady && playlist.isNotEmpty(),
                    ) { Text("Start over") }
                }
                // The rest in the same flow, and all the same kind of button:
                // one filled primary for the thing the page is for, tonal for
                // everything else. A text link among filled buttons was what
                // made this block read as three unrelated rows of controls.
                TonalButton(onClick = { DownloadCoordinator.enqueue(context, playlist) }) {
                    Icon(Icons.Default.Download, contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Download show")
                }
                TonalButton(onClick = { showMatch = true }) {
                    Icon(Icons.Default.Edit, contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Change match")
                }
                // The provider's own page for this title: where a viewer goes to
                // check that the thing the app matched is the thing they have.
                title.externalUrl?.let { url ->
                    EdgedButton(onClick = {
                        runCatching {
                            context.startActivity(Intent(Intent.ACTION_VIEW, url.toUri()))
                        }.onFailure(onPreferenceError)
                    }) { Text("More on ${title.metadataProvider?.displayName() ?: "the provider"}") }
                }
            }
            // Said plainly, because it changes what a re-match will do: a
            // hand-picked match is not overwritten by an automatic pass.
            if (title.manualMatch) {
                Text(
                    "Matched by hand",
                    style = MaterialTheme.typography.labelMedium,
                    modifier = Modifier.padding(top = 8.dp),
                )
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
            TonalButton(onClick = {
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
            val isExpanded = expandedSeasons.contains(season.label)
            item(key = "season/${season.label}") {
                // A header, not a row with a button parked on the end of it: the
                // label and its count are the only things that grow, and the one
                // action is an icon of fixed width, so every season header down
                // the page starts and ends in the same place.
                Row(
                    Modifier
                        .fillMaxWidth()
                        .padding(top = 12.dp)
                        .clip(MaterialTheme.shapes.medium)
                        .clickable {
                            expandedSeasons = if (isExpanded) expandedSeasons - season.label else expandedSeasons + season.label
                        }
                        .padding(vertical = 8.dp, horizontal = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(
                        if (isExpanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
                        contentDescription = if (isExpanded) "Collapse" else "Expand",
                    )
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        Text(
                            season.label,
                            style = MaterialTheme.typography.titleMedium,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        val watched = season.episodes.count(EpisodeRecord::watched)
                        Text(
                            "${season.episodes.size} " +
                                (if (season.episodes.size == 1) "episode" else "episodes") +
                                if (watched > 0) " · $watched watched" else "",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    IconButton(onClick = { DownloadCoordinator.enqueue(context, season.episodes) }) {
                        Icon(
                            Icons.Default.Download,
                            contentDescription = "Download ${season.label}",
                        )
                    }
                }
            }
            if (isExpanded) {
                itemsIndexed(season.episodes, key = { _, it -> it.linkId }) { position, episode ->
                    EpisodeRow(
                        episode = episode,
                        numbering = episode.numbering(season.number, position),
                        download = downloads.firstOrNull {
                            it.shareId == episode.shareId && it.linkId == episode.linkId
                        },
                        playerReady = playerReady,
                        onPlay = { onPlay(playlist, playlist.indexOfFirst { it.linkId == episode.linkId }) },
                        onDownload = { DownloadCoordinator.enqueue(context, episode) },
                        onPause = { DownloadCoordinator.pause(context, it) },
                        onResume = { DownloadCoordinator.resume(context, it) },
                        onSetWatched = { onSetWatched(episode, it) },
                    )
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

/**
 * `S03E38`, `E38`, or the row's place in the season.
 *
 * Mirrors `Episode::numbering` in `pstr-core`, plus the desktop client's
 * fallback to the season folder's number. The last case is deliberately *not*
 * printed as an episode number — nothing in the name numbered this file, and
 * `#4` says "fourth in the list" where `E04` would be a claim about the show.
 * Some ordinal is required regardless: a season of rows that all read the same
 * is a season the viewer cannot navigate at all.
 */
private fun EpisodeRecord.numbering(fallbackSeason: UInt?, position: Int): String {
    val season = this.season ?: fallbackSeason
    val number = this.number
    return when {
        season != null && number != null -> "S%02dE%02d".format(season.toInt(), number.toInt())
        number != null -> "E%02d".format(number.toInt())
        else -> "#${position + 1}"
    }
}

/**
 * One episode: play it, see where it was left, and drive its download.
 *
 * The row *is* the play button — the still, the numbering and the name are one
 * target, which is what lets the trailing controls be two icons in a fixed
 * column rather than a second row of buttons under every episode. Both were
 * needed while the row carried a full-width `Play`: it pushed the download and
 * watched controls into whatever width was left, and forty of those down a
 * season read as forty differently-arranged rows.
 */
@Composable
private fun EpisodeRow(
    episode: EpisodeRecord,
    numbering: String,
    download: RetainedDownload?,
    playerReady: Boolean,
    onPlay: () -> Unit,
    onDownload: () -> Unit,
    onPause: (RetainedDownload) -> Unit,
    onResume: (RetainedDownload) -> Unit,
    onSetWatched: (Boolean) -> Unit,
) {
    val running = download?.status == RetainedDownload.STATUS_RUNNING ||
        download?.status == RetainedDownload.STATUS_QUEUED
    // The provider's name for the episode when there is one: "The Cave of
    // Skulls" says more than the filename it was parsed out of. The filename
    // stays underneath it, and is the whole answer when there is no provider.
    val name = episode.providerName ?: episode.detail
    val detail = episode.detail.takeIf { it != name }
    Card(
        Modifier
            .fillMaxWidth()
            .clickable(enabled = playerReady, onClick = onPlay),
    ) {
        Column(Modifier.padding(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                // With metadata off the provider has no still, and Proton's own
                // thumbnail is a frame of this very episode.
                Box(
                    Modifier
                        .width(96.dp)
                        .aspectRatio(16f / 9f)
                        .clip(MaterialTheme.shapes.small),
                    contentAlignment = Alignment.Center,
                ) {
                    RemoteArtwork(
                        episode.stillUrl,
                        name,
                        Modifier.fillMaxSize(),
                        fallback = episode.thumbnailSource,
                    )
                    // Over the still rather than beside it: the row already
                    // plays on tap, so this says so without spending a column.
                    Icon(
                        Icons.Default.PlayArrow,
                        contentDescription = null,
                        tint = Color.White,
                        modifier = Modifier
                            .size(30.dp)
                            .clip(CircleShape)
                            .background(Color.Black.copy(alpha = 0.45f))
                            .padding(4.dp),
                    )
                }
                Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                    Text(
                        numbering,
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.primary,
                        fontWeight = FontWeight.SemiBold,
                        maxLines = 1,
                    )
                    Text(
                        name,
                        fontWeight = FontWeight.SemiBold,
                        // Seen episodes stay legible but stop competing with the
                        // one the viewer has not watched yet.
                        color = if (episode.watched) {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        } else {
                            MaterialTheme.colorScheme.onSurface
                        },
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                    detail?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                // Watched is a judgement the viewer is allowed to overrule: a
                // half-watched episode they are done with, or one the 90 % rule
                // marked seen because they sat through the credits.
                IconButton(onClick = { onSetWatched(!episode.watched) }) {
                    Icon(
                        if (episode.watched) Icons.Default.CheckCircle else Icons.Outlined.CheckCircle,
                        contentDescription = if (episode.watched) "Mark unwatched" else "Mark watched",
                        tint = if (episode.watched) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.outline
                        },
                    )
                }
                // One slot, always the same width, whatever state the download
                // is in — including "there is nothing to say", where an empty
                // box keeps the rows above and below it aligned.
                Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                    when {
                        episode.offline -> Icon(
                            Icons.Default.CheckCircle,
                            contentDescription = "Saved offline",
                            tint = MaterialTheme.colorScheme.tertiary,
                        )
                        running -> IconButton(onClick = { onPause(download!!) }) {
                            Icon(Icons.Default.Pause, contentDescription = "Pause download")
                        }
                        download != null -> IconButton(onClick = { onResume(download) }) {
                            Icon(Icons.Default.Download, contentDescription = "Resume download")
                        }
                        else -> IconButton(onClick = onDownload) {
                            Icon(Icons.Default.Download, contentDescription = "Download")
                        }
                    }
                }
            }
            // Where the episode was left, under the row it belongs to.
            episode.progress?.takeIf { !episode.watched }?.let { progress ->
                AccentProgress(
                    progress = { progress.toFloat().coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                )
            }
            download?.takeIf { it.total > 0L && !episode.offline }?.let { active ->
                AccentProgress(
                    progress = { (active.downloaded.toFloat() / active.total).coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                    fill = solid(MaterialTheme.colorScheme.tertiary),
                )
                Text(
                    "${active.status.replaceFirstChar { it.uppercase() }} · " +
                        "${formatBytes(active.downloaded.toULong())} of ${formatBytes(active.total.toULong())}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
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
                AccentButton(enabled = term.isNotBlank() && !searching, onClick = {
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
                        EdgedButton(
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
            QuietButton(onClick = {
                scope.launch {
                    runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().forgetMatch(title.key) } }
                        .onSuccess { onChanged() }.onFailure(onError)
                }
            }) { Text("Forget match") }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Close") } },
    )
}
