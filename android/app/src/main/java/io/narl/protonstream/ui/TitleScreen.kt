package io.narl.protonstream.ui

import android.content.Intent
import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.ui.graphics.graphicsLayer
import kotlin.coroutines.cancellation.CancellationException
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.AlertDialog
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
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.runtime.Composable
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
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.MatchRecord
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.material3.FilterChip
import androidx.compose.material.icons.filled.Replay
import androidx.compose.material.icons.filled.DownloadDone
import androidx.compose.material.icons.automirrored.filled.OpenInNew
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.vector.ImageVector

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
    var showMatch by remember(title.key) { mutableStateOf(false) }
    var overviewOpen by remember(title.key) { mutableStateOf(false) }

    // Predictive back: the page shrinks towards the library as the gesture
    // is dragged, and springs back if it is let go of, so the viewer sees
    // where back goes before committing to it.
    var backProgress by remember { mutableFloatStateOf(0f) }
    PredictiveBackHandler { gesture ->
        try {
            gesture.collect { backProgress = it.progress }
            onBack()
        } catch (cancelled: CancellationException) {
            backProgress = 0f
            throw cancelled
        }
    }

    // Display order across seasons: what previous/next and autoplay walk, and
    // the same order the desktop client uses.
    val playlist = remember(title) { title.seasons.flatMap(SeasonRecord::episodes) }
    val nextUp = remember(playlist) { nextUpIndex(playlist) }
    val upNext = playlist.getOrNull(nextUp)
    // One season on screen at a time, opened on the one Play would land in: a
    // forty-episode show as stacked sections is one long scroll to the part
    // that matters.
    var seasonIndex by remember(title.key) {
        mutableStateOf(
            title.seasons.indexOfFirst { season -> season.episodes.any { it.linkId == upNext?.linkId } }
                .coerceAtLeast(0),
        )
    }
    val season = title.seasons.getOrNull(seasonIndex)
    // Live download state per episode, so a row can show progress and be paused
    // without a trip to the Downloads tab.
    val work by WorkManager.getInstance(context)
        .getWorkInfosByTagLiveData(DownloadCoordinator.TAG).observeAsState(emptyList())
    val downloads = remember(work) { DownloadStateStore(context).records() }

    // On a tablet the page keeps a phone's reading width, centred: a Play
    // button and synopsis lines two thousand pixels wide are read by turning
    // the head. The backdrop still spans the window, but no taller than about
    // half of it, or a landscape tablet opened on a picture and nothing else.
    BoxWithConstraints(
        Modifier
            .fillMaxSize()
            .graphicsLayer {
                val scale = 1f - 0.1f * backProgress
                scaleX = scale
                scaleY = scale
                shape = RoundedCornerShape((32 * backProgress).dp)
                clip = backProgress > 0f
            }
            .padding(padding),
    ) {
        val side = ((maxWidth - READABLE_WIDTH) / 2).coerceAtLeast(0.dp)
        val backdropHeight = minOf(maxWidth * 9f / 16f, maxHeight * 0.55f)
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            item(key = "backdrop") {
                // Edge to edge, fading into the page: the art is the first thing
                // the page says, and a framed thumbnail with a margin round it read
                // as one card among the many below it.
                Box(Modifier.sharedArt(title.key).fillMaxWidth().height(backdropHeight)) {
                    RemoteArtwork(
                        title.backdropUrl ?: title.posterUrl,
                        title.canonicalName ?: title.name,
                        Modifier.fillMaxSize(),
                        fallback = title.thumbnailSource,
                        labelled = false,
                    )
                    Box(
                        Modifier.fillMaxSize().background(
                            Brush.verticalGradient(
                                0.45f to Color.Transparent,
                                1f to MaterialTheme.colorScheme.background,
                            ),
                        ),
                    )
                    IconButton(
                        onClick = onBack,
                        modifier = Modifier
                            .statusBarsPadding()
                            .padding(8.dp)
                            .clip(CircleShape)
                            .background(Color.Black.copy(alpha = 0.4f)),
                    ) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back to library", tint = Color.White)
                    }
                }
            }
            item(key = "heading") {
                Column(Modifier.padding(horizontal = side + 16.dp)) {
                    Text(title.canonicalName ?: title.name, style = MaterialTheme.typography.headlineMedium)
                    title.originalName?.takeIf { it != title.canonicalName }?.let {
                        Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Text(
                        listOfNotNull(
                            (title.metadataYear ?: title.year)?.toString(),
                            title.rating?.let { "★ %.1f".format(it) },
                            title.genres.takeIf { it.isNotEmpty() }?.take(3)?.joinToString(", "),
                            "${title.watchedCount} of ${title.episodeCount} watched".takeIf { title.episodeCount > 1uL },
                        ).joinToString("  ·  "),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 6.dp),
                    )
                    // The one thing the page is for, full width and naming what it
                    // will play: "Resume" alone left the viewer guessing which
                    // episode of forty it meant.
                    AccentButton(
                        onClick = { onPlay(playlist, nextUp) },
                        enabled = playerReady && playlist.isNotEmpty(),
                        modifier = Modifier.fillMaxWidth().padding(top = 16.dp),
                    ) {
                        Icon(Icons.Default.PlayArrow, contentDescription = null, modifier = Modifier.size(20.dp))
                        Spacer(Modifier.width(8.dp))
                        Text(
                            when {
                                !playerReady -> "Player loading…"
                                upNext == null -> "Play"
                                upNext.resumeAt != null -> "Resume ${upNext.label}"
                                title.watchedCount > 0uL -> "Continue with ${upNext.label}"
                                playlist.size == 1 -> "Play"
                                else -> "Play ${upNext.label}"
                            },
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    // Everything else as labelled icons in one row: four actions of
                    // three button styles used to wrap over three lines and bury
                    // the episodes under them.
                    Row(Modifier.fillMaxWidth().padding(top = 12.dp)) {
                        // Only where it would do something different: an unstarted
                        // show already starts at the beginning.
                        if (upNext?.resumeAt != null || title.watchedCount > 0uL) {
                            TitleAction(Icons.Default.Replay, "Start over", Modifier.weight(1f), enabled = playerReady) {
                                onPlay(playlist, 0)
                            }
                        }
                        TitleAction(Icons.Default.Download, "Download", Modifier.weight(1f)) {
                            DownloadCoordinator.enqueue(context, playlist)
                        }
                        TitleAction(Icons.Default.Edit, "Match", Modifier.weight(1f)) { showMatch = true }
                        // The provider's own page for this title: where a viewer goes
                        // to check that the thing the app matched is what they have.
                        title.externalUrl?.let { url ->
                            TitleAction(
                                Icons.AutoMirrored.Filled.OpenInNew,
                                title.metadataProvider?.displayName() ?: "Provider",
                                Modifier.weight(1f),
                            ) {
                                runCatching {
                                    context.startActivity(Intent(Intent.ACTION_VIEW, url.toUri()))
                                }.onFailure(onPreferenceError)
                            }
                        }
                    }
                    title.overview?.let { overview ->
                        // Three lines, then a tap for the rest: a synopsis with its
                        // source notes ran to a screen and a half on a phone.
                        Text(
                            overview.trim(),
                            maxLines = if (overviewOpen) Int.MAX_VALUE else 3,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier
                                .padding(top = 12.dp)
                                .clickable { overviewOpen = !overviewOpen },
                        )
                    }
                    // Said plainly, because it changes what a re-match will do: a
                    // hand-picked match is not overwritten by an automatic pass.
                    if (title.manualMatch) {
                        Text(
                            "Matched by hand",
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(top = 8.dp),
                        )
                    }
                }
            }
            item(key = "seasons") {
                Row(
                    Modifier.fillMaxWidth().padding(start = side + 16.dp, end = side + 4.dp, top = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    LazyRow(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        itemsIndexed(title.seasons, key = { _, it -> it.label }) { index, entry ->
                            FilterChip(
                                selected = index == seasonIndex,
                                onClick = { seasonIndex = index },
                                label = { Text(entry.label) },
                            )
                        }
                    }
                    season?.let {
                        IconButton(onClick = { DownloadCoordinator.enqueue(context, it.episodes) }) {
                            Icon(Icons.Default.Download, contentDescription = "Download ${it.label}")
                        }
                    }
                }
            }
            season?.let { shown ->
                itemsIndexed(shown.episodes, key = { _, it -> it.linkId }) { position, episode ->
                    Box(Modifier.padding(horizontal = side)) {
                        EpisodeRow(
                            episode = episode,
                            numbering = episode.numbering(shown.number, position),
                            position = position,
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

/** How wide the page's text and controls run on a window wider than a phone. */
private val READABLE_WIDTH = 720.dp

/** One of the title's secondary actions: an icon with its name under it. */
@Composable
private fun TitleAction(
    icon: ImageVector,
    label: String,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    onClick: () -> Unit,
) {
    Column(
        modifier
            .clip(MaterialTheme.shapes.medium)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(vertical = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(icon, contentDescription = null)
        Text(
            label,
            style = MaterialTheme.typography.labelMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 4.dp),
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
 * The row *is* the play button — the still, the number and the name are one
 * target, which is what lets the trailing controls be two icons in a fixed
 * column rather than a second row of buttons under every episode.
 */
@Composable
private fun EpisodeRow(
    episode: EpisodeRecord,
    numbering: String,
    position: Int,
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
    val heading = episode.heading(position)
    Column(
        Modifier
            .fillMaxWidth()
            .clickable(enabled = playerReady, onClick = onPlay)
            .padding(horizontal = 16.dp, vertical = 6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            // With metadata off the provider has no still, and Proton's own
            // thumbnail is a frame of this very episode. Where it was left sits
            // on the still's lower edge, the way every streaming app shows it.
            Box(
                Modifier
                    .width(128.dp)
                    .aspectRatio(16f / 9f)
                    .clip(MaterialTheme.shapes.small),
                contentAlignment = Alignment.Center,
            ) {
                RemoteArtwork(
                    episode.stillUrl,
                    heading,
                    Modifier.fillMaxSize(),
                    fallback = episode.thumbnailSource,
                    labelled = false,
                )
                episode.progress?.takeIf { !episode.watched }?.let { progress ->
                    AccentProgress(
                        progress = { progress.toFloat().coerceIn(0f, 1f) },
                        modifier = Modifier.fillMaxWidth().align(Alignment.BottomCenter),
                    )
                }
            }
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(
                    heading,
                    style = MaterialTheme.typography.titleSmall,
                    // Seen episodes stay legible but stop competing with the
                    // one the viewer has not watched yet.
                    color = if (episode.watched) {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    } else {
                        MaterialTheme.colorScheme.onSurface
                    },
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    listOfNotNull(
                        numbering,
                        episode.size?.let(::formatBytes),
                        episode.airDate,
                    ).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(top = 2.dp),
                )
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
                        Icons.Default.DownloadDone,
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
        episode.providerOverview?.let {
            Text(
                it,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 6.dp),
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

@Composable
internal fun ChangeMatchDialog(
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
