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
import io.narl.protonstream.ui.theme.AppearanceState
import uniffi.pstr_android.ArrangementRecord
import uniffi.pstr_android.LibrarySort
import uniffi.pstr_android.ShareRecord
import uniffi.pstr_android.TitleRecord
import uniffi.pstr_android.OfflineRecord
import uniffi.pstr_android.MatchSummary
import uniffi.pstr_android.MetadataProvider
import uniffi.pstr_android.MetadataSettingsRecord
import uniffi.pstr_android.StorageUsageRecord
import uniffi.pstr_android.EpisodeRecord
import uniffi.pstr_android.WatchStateRecord
import uniffi.pstr_android.AccountState
import uniffi.pstr_android.SignInOutcome
import uniffi.pstr_android.SyncRecord
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

data class AppUiState(
    val loading: Boolean = true,
    val refreshing: Boolean = false,
    val query: String = "",
    val titles: List<TitleRecord> = emptyList(),
    /**
     * The grid: [titles] ordered, each franchise folded into one tile when
     * [grouped], and the shelves above it. Null until the bridge has answered,
     * when the grid falls back to [titles] as they are.
     */
    val arrangement: ArrangementRecord? = null,
    val sort: LibrarySort = LibrarySort.NAME,
    val grouped: Boolean = true,
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
    /**
     * What is running in the background right now, said in a banner for as
     * long as it runs: an add, a crawl, a match, a sync. Without it a
     * minute-long crawl looked like a tap that did nothing.
     */
    val activity: String? = null,
    /** What Undo on [message] puts back, when the message offers one. */
    val undo: List<WatchSnapshot>? = null,
    val account: AccountUiState = AccountUiState(),
)

/** The Proton account: where a sign-in stands, and how sync is doing. */
data class AccountUiState(
    /** Null until the bridge has said. */
    val state: AccountState? = null,
    /** A sign-in step is out and not answered yet. */
    val busy: Boolean = false,
    /** Why the last sign-in step was refused. */
    val error: String? = null,
    /** The CAPTCHA page Proton wants solved before it accepts the sign-in. */
    val verificationUrl: String? = null,
    /** When watch history last synced, in epoch milliseconds. */
    val syncedAt: Long? = null,
    /** Positions the last sync took from other devices. */
    val applied: Int = 0,
    /** Why the last sync failed, until one succeeds. */
    val syncError: String? = null,
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
    /** Launch and coming back both ask; one crawl answers. */
    private var catchingUp = false

    init {
        if (!NativeRuntime.tlsReady) {
            mutableState.update {
                it.copy(message = "Certificate verification is unavailable; Proton requests will fail")
            }
        }
        reload()
        catchUp()
        viewModelScope.launch {
            val account = runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().accountState() } }
                .getOrDefault(AccountState.SignedOut)
            updateAccount { it.copy(state = account) }
            // While the app is alive, on top of after sign-in and whenever the
            // player closes. Cheap when nothing changed: one folder listing,
            // and no upload.
            while (isActive) {
                syncWatchHistory()
                // A sync WatchSyncWorker ran while the app was away may have
                // brought shares this one has not crawled.
                catchUp()
                delay(SYNC_INTERVAL_MS)
            }
        }
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

    /** Reorder the grid, or fold or unfold its franchises. */
    fun arrange(sort: LibrarySort, grouped: Boolean) {
        mutableState.update { it.copy(sort = sort, grouped = grouped) }
        viewModelScope.launch { reloadLibrary(mutableState.value.query) }
    }

    fun search(query: String) {
        mutableState.update { it.copy(query = query) }
        searchQuery.value = query
    }

    fun refresh() {
        viewModelScope.launch {
            busy("Refreshing the library…") {
                mutableState.update { it.copy(message = null) }
                runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().crawl(null) } }
                    .onFailure { error -> mutableState.update { it.copy(message = error.message) } }
            }
            reload()
        }
    }

    /**
     * Say [what] in the activity banner while [work] runs. Nested work keeps
     * the outer line once the inner one is done.
     */
    private suspend fun busy(what: String, work: suspend () -> Unit) {
        val outer = mutableState.value.activity
        mutableState.update { it.copy(activity = what, refreshing = true) }
        try {
            work()
        } finally {
            mutableState.update { it.copy(activity = outer, refreshing = outer != null) }
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
            val name = mutableState.value.shares.firstOrNull { it.id == id }?.name ?: "the share"
            busy("Reading $name…") {
                mutableState.update { it.copy(message = null) }
                runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().crawl(id) } }
                    .onFailure { error -> mutableState.update { it.copy(message = error.message) } }
            }
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
        viewModelScope.launch { match(force) }
    }

    private suspend fun match(force: Boolean) {
        busy("Looking up titles…") {
            runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().matchTitles(force) } }
                .onSuccess { summary -> mutableState.update { it.copy(message = describe(summary)) } }
                .onFailure { error -> mutableState.update { it.copy(message = error.message) } }
        }
        reloadNow()
    }

    /**
     * Add a link, and return why it was refused, if it was.
     *
     * The link is opened before anything is stored, so the form can stay up
     * with the reason under it and a retry is a retry — not "already in the
     * library" for a share that never opened. Only the new share is crawled:
     * another share that fails to open must not make this one look broken.
     */
    suspend fun addShare(name: String, url: String, password: String?): String? {
        val share = runCatching {
            withContext(Dispatchers.IO) {
                NativeRuntime.engine().addShare(name, url, password?.takeIf(String::isNotBlank))
            }
        }.getOrElse { return it.message ?: "That link could not be added" }
        reload()
        viewModelScope.launch { crawlAdded(listOf(share.id), announce = true) }
        return null
    }

    /**
     * Crawl shares that are new here — added on this device or by sync — then
     * match them when enrichment is on.
     */
    private suspend fun crawlAdded(ids: List<String>, announce: Boolean) {
        if (ids.isEmpty()) return
        val before = mutableState.value.titles.map { it.key }.toSet()
        val failed = mutableListOf<String>()
        for (id in ids) {
            val name = mutableState.value.shares.firstOrNull { it.id == id }?.name ?: "the new share"
            busy("Reading $name…") {
                runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().crawl(id) } }
                    .onFailure { failed += "$name: ${it.message}" }
            }
        }
        reloadNow()
        if (failed.isNotEmpty()) {
            mutableState.update { it.copy(message = failed.joinToString("\n")) }
            return
        }
        if (mutableState.value.metadataSettings.enabled) match(force = false)
        if (announce) {
            val added = (mutableState.value.titles.map { it.key }.toSet() - before).size
            mutableState.update { it.copy(message = if (added == 1) "1 title added" else "$added titles added") }
        }
    }

    /**
     * Crawl shares the catalog has nothing of: brought by a sync that ran in
     * [io.narl.protonstream.sync.WatchSyncWorker], or an add whose crawl never
     * finished.
     */
    fun catchUp() {
        if (catchingUp) return
        catchingUp = true
        viewModelScope.launch {
            try {
                val ids = runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().uncrawledShares() } }
                    .getOrDefault(emptyList())
                crawlAdded(ids, announce = false)
            } finally {
                catchingUp = false
            }
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

    private fun updateAccount(change: (AccountUiState) -> AccountUiState) =
        mutableState.update { it.copy(account = change(it.account)) }

    /**
     * Start signing in, or start again with [verificationToken] once the
     * CAPTCHA from a first try is solved.
     */
    fun signIn(username: String, password: String, verificationToken: String? = null) =
        signInStep { engine -> engine.signIn(username, password, verificationToken) }

    fun submitSecondFactor(code: String) = signInStep { engine -> engine.submitSecondFactor(code) }

    fun submitMailboxPassword(password: String) =
        signInStep(keepStepOnFailure = true) { engine -> engine.submitMailboxPassword(password) }

    fun cancelSignIn() {
        updateAccount { AccountUiState(state = AccountState.SignedOut) }
        viewModelScope.launch { runCatching { NativeRuntime.engine().cancelSignIn() } }
    }

    /** The CAPTCHA page closed without a token. */
    fun dismissVerification() = updateAccount { it.copy(verificationUrl = null, busy = false) }

    private fun signInStep(
        keepStepOnFailure: Boolean = false,
        step: suspend (uniffi.pstr_android.AndroidEngine) -> SignInOutcome,
    ) {
        updateAccount { it.copy(busy = true, error = null, verificationUrl = null) }
        viewModelScope.launch {
            runCatching { withContext(Dispatchers.IO) { step(NativeRuntime.engine()) } }
                .onSuccess { outcome ->
                    when (outcome) {
                        is SignInOutcome.Done -> {
                            updateAccount {
                                AccountUiState(state = AccountState.SignedIn(outcome.username))
                            }
                            mutableState.update { it.copy(message = "Signed in as ${outcome.username}") }
                            syncWatchHistory()
                            reload()
                        }
                        SignInOutcome.SecondFactor ->
                            updateAccount { it.copy(state = AccountState.SecondFactor, busy = false) }
                        SignInOutcome.MailboxPassword ->
                            updateAccount { it.copy(state = AccountState.MailboxPassword, busy = false) }
                        is SignInOutcome.HumanVerification ->
                            updateAccount { it.copy(verificationUrl = outcome.url) }
                    }
                }
                .onFailure { error ->
                    updateAccount {
                        it.copy(
                            busy = false,
                            error = error.message ?: "Sign-in failed",
                            // A refused code spends the half-made session.
                            state = if (keepStepOnFailure) it.state else AccountState.SignedOut,
                        )
                    }
                }
        }
    }

    fun signOut() {
        viewModelScope.launch {
            runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().signOut() } }
                .onFailure(::reportError)
            updateAccount { AccountUiState(state = AccountState.SignedOut) }
            reload()
        }
    }

    /** Sync now, from a tap: says so when it is done. */
    fun syncNow() {
        viewModelScope.launch {
            if (syncWatchHistory()) {
                val applied = mutableState.value.account.applied
                mutableState.update {
                    it.copy(
                        message = if (applied > 0) {
                            "Watch history synced, $applied from other devices"
                        } else {
                            "Watch history is up to date"
                        },
                    )
                }
            }
        }
    }

    /**
     * Pull other devices' positions in and push this one's out. Quiet: it runs
     * on a timer, and a failure is shown on the account card rather than as a
     * snackbar every five minutes of an offline evening. Returns whether it
     * synced.
     */
    private suspend fun syncWatchHistory(): Boolean {
        if (mutableState.value.account.state !is AccountState.SignedIn) return false
        return runCatching { withContext(Dispatchers.IO) { NativeRuntime.engine().syncWatchHistory() } }
            .fold(
                onSuccess = { report ->
                    if (report == null) return false
                    updateAccount {
                        it.copy(
                            syncedAt = System.currentTimeMillis(),
                            applied = report.applied.toInt(),
                            syncError = null,
                        )
                    }
                    takeSynced(report)
                    true
                },
                onFailure = { error ->
                    val state = runCatching { NativeRuntime.engine().accountState() }.getOrNull()
                    updateAccount { it.copy(state = state ?: it.state, syncError = error.message) }
                    // Signed out by the engine: the session ended, and that is
                    // worth interrupting for, unlike a dropped connection.
                    if (state == AccountState.SignedOut) reportError(error)
                    false
                },
            )
    }

    /**
     * What another device changed: its shares crawled or dropped here, its
     * settings repainted.
     */
    private suspend fun takeSynced(report: SyncRecord) {
        if (report.sharesRemoved.isNotEmpty()) {
            withContext(Dispatchers.IO) {
                val downloads = DownloadStateStore(appContext)
                report.sharesRemoved.forEach { id ->
                    workManager.cancelAllWorkByTag(DownloadCoordinator.shareTag(id)).result.get()
                    downloads.removeShare(id)
                }
            }
        }
        if (report.settingsChanged) AppearanceState.reload()
        if (report.applied > 0u || report.titles > 0u || report.settingsChanged || report.sharesRemoved.isNotEmpty()) {
            reloadNow()
        }
        if (report.sharesAdded.isNotEmpty()) {
            crawlAdded(report.sharesAdded, announce = false)
            val count = report.sharesAdded.size
            mutableState.update {
                it.copy(message = if (count == 1) "A share arrived from another device" else "$count shares arrived from other devices")
            }
        }
    }

    /** After the player closes: where it stopped is what another device wants next. */
    fun playerClosed() {
        reload()
        viewModelScope.launch { syncWatchHistory() }
    }

    fun addAccountFolder(name: String, volumeId: String, linkId: String) {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) { NativeRuntime.engine().addAccountFolder(name, volumeId, linkId) }
            }.onSuccess { share ->
                reload()
                crawlAdded(listOf(share.id), announce = true)
            }.onFailure(::reportError)
        }
    }

    /**
     * Every episode of a title watched or unwatched, with an Undo: a whole
     * title's positions are what this throws away, and a mis-tap on a tile's
     * menu should not cost them.
     */
    fun setTitleWatched(title: TitleRecord, watched: Boolean) {
        val episodes = title.seasons.flatMap { it.episodes }
        changeWatch(
            episodes,
            "${title.displayName} marked ${if (watched) "watched" else "unwatched"}",
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
            "${title.displayName} removed from Continue watching",
        ) { engine, target, before ->
            engine.saveWatchState(target.shareId, target.linkId, 0.0, before?.durationSecs, false)
        }
    }

    /**
     * Off the history page: the position and the watched mark both go, which
     * is "never played", with an Undo.
     */
    fun removeFromHistory(title: TitleRecord, episode: EpisodeRecord) {
        changeWatch(
            listOf(episode),
            "${episode.label} of ${title.displayName} removed from history",
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

    /**
     * Store the enrichment settings, and return why not, if they were not.
     *
     * The page shows the new settings as soon as they are stored; the match
     * run that follows can take minutes and says so in the activity banner.
     */
    suspend fun saveMetadataSettings(enabled: Boolean, provider: MetadataProvider, language: String, apiKey: String): String? {
        val stored = runCatching {
            withContext(Dispatchers.IO) {
                val engine = NativeRuntime.engine()
                if (provider == MetadataProvider.TMDB && apiKey.isNotBlank()) {
                    engine.setMetadataApiKey(provider, apiKey)
                }
                engine.setMetadataSettings(MetadataSettingsRecord(enabled, provider, language, true))
                engine.metadataSettings()
            }
        }.getOrElse { return it.message ?: "The settings could not be saved" }
        mutableState.update { it.copy(metadataSettings = stored) }
        reloadNow()
        if (enabled) viewModelScope.launch { match(force = false) }
        return null
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
        viewModelScope.launch { reloadNow() }
    }

    /** [reload], for a caller that goes on to read what it loaded. */
    private suspend fun reloadNow() {
        runCatching {
            withContext(Dispatchers.IO) {
                val engine = NativeRuntime.engine()
                // Here rather than inside `library()`, which is read-only
                // and cached: a full reload is the one place that has
                // already paid for a walk of the offline files.
                engine.pruneOfflineFiles()
                val current = mutableState.value
                val query = current.query.takeIf(String::isNotBlank)
                Reloaded(
                    engine.shares(),
                    engine.library(query),
                    engine.arrangement(query, current.sort, current.grouped),
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
                    arrangement = result.arrangement,
                    offline = result.offline,
                    metadataSettings = result.metadataSettings,
                    storage = result.storage,
                ) }
            }
            .onFailure { error ->
                mutableState.update { it.copy(loading = false, message = error.message) }
            }
    }

    private suspend fun reloadLibrary(query: String) {
        val current = mutableState.value
        runCatching {
            withContext(Dispatchers.IO) {
                val engine = NativeRuntime.engine()
                val search = query.takeIf(String::isNotBlank)
                engine.library(search) to engine.arrangement(search, current.sort, current.grouped)
            }
        }.onSuccess { (titles, arrangement) ->
            mutableState.update { it.copy(titles = titles, arrangement = arrangement) }
        }.onFailure { error -> mutableState.update { it.copy(message = error.message) } }
    }

    private companion object {
        const val SYNC_INTERVAL_MS = 5 * 60 * 1000L
    }

    class Factory(private val context: Context, private val workManager: WorkManager) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T = AppViewModel(context, workManager) as T
    }
}

private data class Reloaded(
    val shares: List<ShareRecord>,
    val titles: List<TitleRecord>,
    val arrangement: ArrangementRecord,
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
