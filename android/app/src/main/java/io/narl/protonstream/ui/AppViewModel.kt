package io.narl.protonstream.ui

import android.content.Context
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.Observer
import androidx.work.WorkInfo
import androidx.work.WorkManager
import io.narl.protonstream.download.DownloadCoordinator
import io.narl.protonstream.download.DownloadStateStore
import io.narl.protonstream.download.RetainedDownload
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.withContext
import io.narl.protonstream.native.NativeRuntime
import uniffi.pstr_android.ShareRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.OfflineRecord
import uniffi.pstr_android.MatchSummary
import uniffi.pstr_android.MetadataProvider
import uniffi.pstr_android.MetadataSettingsRecord
import uniffi.pstr_android.StorageUsageRecord
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.WatchStateRecord

data class AppUiState(
    val loading: Boolean = true,
    val refreshing: Boolean = false,
    val query: String = "",
    val titles: List<TitleRecord> = emptyList(),
    val shares: List<ShareRecord> = emptyList(),
    val offline: List<OfflineRecord> = emptyList(),
    val metadataSettings: MetadataSettingsRecord = MetadataSettingsRecord(
        enabled = false,
        provider = MetadataProvider.ANI_LIST,
        language = "en",
        ready = true,
    ),
    val storage: StorageUsageRecord = StorageUsageRecord(0uL, 0uL, 0uL, 0uL),
    val message: String? = null,
    /** What Undo on [message] puts back, when the message offers one. */
    val undo: List<WatchSnapshot>? = null,
)

/**
 * One episode's watch state as it was before a change the viewer can undo.
 * Null [state] means it had never been played.
 */
data class WatchSnapshot(val shareId: String, val linkId: String, val state: WatchStateRecord?)

@OptIn(FlowPreview::class)
class AppViewModel(context: Context, private val workManager: WorkManager) : ViewModel() {
    private val appContext = context.applicationContext
    private val mutableState = MutableStateFlow(AppUiState())
    private val searchQuery = MutableStateFlow("")
    val state: StateFlow<AppUiState> = mutableState.asStateFlow()

    init {
        if (!NativeRuntime.tlsReady) {
            mutableState.update {
                it.copy(message = "Certificate verification is unavailable; Proton requests will fail")
            }
        }
        reload()
        viewModelScope.launch {
            searchQuery.debounce(300).distinctUntilChanged().collect { reloadLibrary(it) }
        }
        viewModelScope.launch {
            workManager.workInfosByTagFlow(DownloadCoordinator.TAG)
                .map { work -> work.filter { it.state.isFinished }.map { it.id }.toSet() }
                .distinctUntilChanged()
                .drop(1)
                .collect { reload() }
        }
    }

    fun search(query: String) {
        mutableState.update { it.copy(query = query) }
        searchQuery.value = query
    }

    fun refresh() {
        viewModelScope.launch {
            mutableState.update { it.copy(refreshing = true, message = null) }
            runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().crawl(null) } }
                .onFailure { error -> mutableState.update { it.copy(message = error.message) } }
            mutableState.update { it.copy(refreshing = false) }
            reload()
        }
    }

    /**
     * Recrawl one share rather than the whole library.
     *
     * A viewer who has just added files to one link should not have to pay for
     * a walk of every other one — and a share whose link has expired makes a
     * whole-library refresh fail, which used to leave no way to refresh the
     * shares that still work.
     */
    fun refreshShare(id: String) {
        viewModelScope.launch {
            mutableState.update { it.copy(refreshing = true, message = null) }
            runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().crawl(id) } }
                .onFailure { error -> mutableState.update { it.copy(message = error.message) } }
            mutableState.update { it.copy(refreshing = false) }
            reload()
        }
    }

    /**
     * Run enrichment again.
     *
     * With [force] every title is looked up afresh, including ones that already
     * matched — the way out of a library where a provider's early answers were
     * wrong, or where a match was made against a provider that has since been
     * changed.
     */
    fun matchTitles(force: Boolean) {
        viewModelScope.launch {
            mutableState.update { it.copy(refreshing = true, message = null) }
            runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().matchTitles(force) } }
                .onSuccess { summary -> mutableState.update { it.copy(message = describe(summary)) } }
                .onFailure { error -> mutableState.update { it.copy(message = error.message) } }
            mutableState.update { it.copy(refreshing = false) }
            reload()
        }
    }

    fun addShare(name: String, url: String, password: String?) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    engine.addShare(name, url, password?.takeIf(String::isNotBlank))
                    engine.crawl(null)
                }
            }.onFailure { error -> mutableState.update { it.copy(message = error.message) } }
            reload()
        }
    }

    /**
     * Re-supply the link behind a share whose stored secret cannot be read.
     *
     * The Keystore key is invalidated by a lockscreen change or a device
     * restore, and after that every open of every share fails — including the
     * read inside "remove this share", so the obvious way out is closed too.
     * Re-entering the link rewrites the secret and leaves the catalog and the
     * offline files alone.
     */
    fun repairShare(id: String, url: String, password: String?) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    engine.repairShare(id, url, password?.takeIf(String::isNotBlank))
                    engine.crawl(id)
                }
            }.onFailure(::reportError)
            reload()
        }
    }

    fun removeShare(id: String) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    // Wait for WorkManager cancellation before Rust removes
                    // catalog/files. Rust also rejects any late publication.
                    workManager.cancelAllWorkByTag(DownloadCoordinator.shareTag(id)).result.get()
                    val downloads = DownloadStateStore(appContext)
                    val engine = NativeRuntime.engine()
                    downloads.records().filter { it.shareId == id }.forEach {
                        engine.removeOfflineEpisode(it.shareId, it.linkId)
                    }
                    downloads.removeShare(id)
                    engine.removeShare(id)
                }
            }
                .onFailure { error -> mutableState.update { it.copy(message = error.message) } }
            reload()
        }
    }

    fun dismissMessage() = mutableState.update { it.copy(message = null, undo = null) }

    /**
     * Every episode of a title watched or unwatched, with an Undo: a whole
     * title's positions are what this throws away, and a mis-tap on a tile's
     * menu should not cost them.
     */
    fun setTitleWatched(title: TitleRecord, watched: Boolean) {
        val episodes = title.seasons.flatMap { it.episodes }
        changeWatch(
            episodes,
            "${title.canonicalName ?: title.name} marked ${if (watched) "watched" else "unwatched"}",
        ) { engine, episode, before ->
            val duration = before?.durationSecs
            engine.saveWatchState(
                episode.shareId,
                episode.linkId,
                if (watched) duration ?: 0.0 else 0.0,
                duration,
                watched,
            )
        }
    }

    /**
     * Off Continue watching by forgetting where the episode stopped — what the
     * desktop does — with an Undo that puts the position back.
     */
    fun forgetPosition(title: TitleRecord, episode: EpisodeRecord) {
        changeWatch(
            listOf(episode),
            "${title.canonicalName ?: title.name} removed from Continue watching",
        ) { engine, target, before ->
            engine.saveWatchState(target.shareId, target.linkId, 0.0, before?.durationSecs, false)
        }
    }

    /** Put back what the last undoable change replaced. */
    fun undo() {
        val snapshots = mutableState.value.undo ?: return
        mutableState.update { it.copy(message = null, undo = null) }
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    snapshots.forEach { snapshot ->
                        val state = snapshot.state
                        engine.saveWatchState(
                            snapshot.shareId,
                            snapshot.linkId,
                            state?.positionSecs ?: 0.0,
                            state?.durationSecs,
                            state?.watched ?: false,
                        )
                    }
                }
            }.onFailure(::reportError)
            reload()
        }
    }

    private fun changeWatch(
        episodes: List<EpisodeRecord>,
        message: String,
        change: (uniffi.pstr_android.AndroidEngine, EpisodeRecord, WatchStateRecord?) -> Unit,
    ) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    episodes.map { episode ->
                        val before = engine.watchState(episode.shareId, episode.linkId)
                        change(engine, episode, before)
                        WatchSnapshot(episode.shareId, episode.linkId, before)
                    }
                }
            }.onSuccess { snapshots ->
                mutableState.update { it.copy(message = message, undo = snapshots) }
            }.onFailure(::reportError)
            reload()
        }
    }

    fun reportError(error: Throwable) {
        mutableState.update { it.copy(message = error.message ?: "Unexpected error") }
    }

    fun saveMetadataSettings(enabled: Boolean, provider: MetadataProvider, language: String, apiKey: String) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    if (provider == MetadataProvider.TMDB && apiKey.isNotBlank()) {
                        engine.setMetadataApiKey(provider, apiKey)
                    }
                    engine.setMetadataSettings(MetadataSettingsRecord(enabled, provider, language, true))
                    if (enabled) engine.matchTitles(false) else null
                }
            }.onSuccess { summary ->
                summary?.let { mutableState.update { state -> state.copy(message = describe(it)) } }
            }.onFailure(::reportError)
            reload()
        }
    }

    fun reloadAfterMetadataChange() = reload()

    /**
     * What an enrichment pass did, in one line.
     *
     * Failures are reported even when most titles matched: a run that quietly
     * dropped a third of the library is the case this counting exists for.
     */
    private fun describe(summary: MatchSummary): String {
        if (summary.matched == 0u && summary.unmatched == 0u && summary.failed == 0u) {
            return "Everything is already matched"
        }
        val parts = mutableListOf("${summary.matched} matched")
        if (summary.unmatched > 0u) parts += "${summary.unmatched} not found"
        if (summary.episodes > 0u) parts += "${summary.episodes} episodes named"
        if (summary.failed > 0u) parts += "${summary.failed} failed"
        return parts.joinToString(", ")
    }

    fun removeOffline(file: OfflineRecord) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) { NativeRuntime.engine().removeOfflineEpisode(file.shareId, file.linkId) }
            }.onFailure { error -> mutableState.update { it.copy(message = error.message) } }
            reload()
        }
    }

    fun pauseDownload(download: RetainedDownload) = DownloadCoordinator.pause(appContext, download)

    fun resumeDownload(download: RetainedDownload) = DownloadCoordinator.resume(appContext, download)

    fun deletePartial(download: RetainedDownload) {
        viewModelScope.launch {
            DownloadCoordinator.cancel(appContext, download)
            runCatching {
                withContext(Dispatchers.IO) {
                    workManager.cancelUniqueWork(
                        DownloadCoordinator.workName(download.shareId, download.linkId),
                    ).result.get()
                    NativeRuntime.engine().removeOfflineEpisode(download.shareId, download.linkId)
                    DownloadStateStore(appContext).remove(download.shareId, download.linkId)
                }
            }.onFailure(::reportError)
            reload()
        }
    }

    /**
     * Persist where an episode was left.
     *
     * The player calls this while it is still up and once more as it leaves, so
     * it runs on the ViewModel's scope rather than the screen's — a save issued
     * on the way out must not be cancelled by the screen going away.
     */
    fun saveProgress(episode: EpisodeRecord, position: Double, duration: Double, watched: Boolean) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    NativeRuntime.engine().saveWatchState(
                        episode.shareId,
                        episode.linkId,
                        position.coerceIn(0.0, duration),
                        duration,
                        watched,
                    )
                }
            }.onFailure(::reportError)
        }
    }

    /**
     * Mark an episode seen, or unseen, without playing it.
     *
     * Unwatching rewinds, matching the desktop client: an episode marked unseen
     * that kept its position would come straight back as "resume at 19:04",
     * which is not what the viewer asked for. Marking one seen puts the position
     * at its duration where one is known, so the progress bar agrees with the
     * tick.
     */
    fun setWatched(episode: EpisodeRecord, watched: Boolean) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    val duration = engine.watchState(episode.shareId, episode.linkId)?.durationSecs
                    engine.saveWatchState(
                        episode.shareId,
                        episode.linkId,
                        if (watched) duration ?: 0.0 else 0.0,
                        duration,
                        watched,
                    )
                }
            }.onFailure(::reportError)
            reload()
        }
    }

    fun removeAllOffline() {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    workManager.cancelAllWorkByTag(DownloadCoordinator.TAG).result.get()
                    NativeRuntime.engine().removeAllOffline()
                    DownloadStateStore(appContext).clear()
                }
            }.onFailure(::reportError)
            reload()
        }
    }

    fun clearBlockCache() {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) { NativeRuntime.engine().clearBlockCache() }
            }.onSuccess { reclaimed ->
                mutableState.update { it.copy(message = "Reclaimed ${formatBytes(reclaimed)} of cache") }
            }.onFailure(::reportError)
            reload()
        }
    }

    private fun reload() {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    // Here rather than inside `library()`, which is read-only
                    // and cached: a full reload is the one place that has
                    // already paid for a walk of the offline files.
                    engine.pruneOfflineFiles()
                    Reloaded(
                        engine.shares(),
                        engine.library(mutableState.value.query.takeIf(String::isNotBlank)),
                        engine.offlineFiles(),
                        engine.metadataSettings(),
                        engine.storageUsage(),
                    )
                }
            }.onSuccess { result ->
                    mutableState.update { it.copy(
                        loading = false,
                        shares = result.shares,
                        titles = result.titles,
                        offline = result.offline,
                        metadataSettings = result.metadataSettings,
                        storage = result.storage,
                    ) }
                }
                .onFailure { error ->
                    mutableState.update { it.copy(loading = false, message = error.message) }
                }
        }
    }

    private suspend fun reloadLibrary(query: String) {
        runCatching {
            withContext(Dispatchers.IO) { NativeRuntime.engine().library(query.takeIf(String::isNotBlank)) }
        }.onSuccess { titles -> mutableState.update { it.copy(titles = titles) } }
            .onFailure { error -> mutableState.update { it.copy(message = error.message) } }
    }

    class Factory(private val context: Context, private val workManager: WorkManager) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T = AppViewModel(context, workManager) as T
    }
}

private data class Reloaded(
    val shares: List<ShareRecord>,
    val titles: List<TitleRecord>,
    val offline: List<OfflineRecord>,
    val metadataSettings: MetadataSettingsRecord,
    val storage: StorageUsageRecord,
)

private fun WorkManager.workInfosByTagFlow(tag: String) = callbackFlow {
    val work = getWorkInfosByTagLiveData(tag)
    val observer = Observer<List<WorkInfo>> { trySend(it).isSuccess }
    work.observeForever(observer)
    awaitClose { work.removeObserver(observer) }
}
