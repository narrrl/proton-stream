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
import uniffi.pstr_android.MetadataProvider
import uniffi.pstr_android.MetadataSettingsRecord

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
    val message: String? = null,
)

@OptIn(FlowPreview::class)
class AppViewModel(context: Context, private val workManager: WorkManager) : ViewModel() {
    private val appContext = context.applicationContext
    private val mutableState = MutableStateFlow(AppUiState())
    private val searchQuery = MutableStateFlow("")
    val state: StateFlow<AppUiState> = mutableState.asStateFlow()

    init {
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

    fun dismissMessage() = mutableState.update { it.copy(message = null) }

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
                    if (enabled) engine.matchTitles(false)
                }
            }.onFailure(::reportError)
            reload()
        }
    }

    fun reloadAfterMetadataChange() = reload()

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

    private fun reload() {
        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val engine = NativeRuntime.engine()
                    Reloaded(
                        engine.shares(),
                        engine.library(mutableState.value.query.takeIf(String::isNotBlank)),
                        engine.offlineFiles(),
                        engine.metadataSettings(),
                    )
                }
            }.onSuccess { result ->
                    mutableState.update { it.copy(
                        loading = false,
                        shares = result.shares,
                        titles = result.titles,
                        offline = result.offline,
                        metadataSettings = result.metadataSettings,
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
)

private fun WorkManager.workInfosByTagFlow(tag: String) = callbackFlow {
    val work = getWorkInfosByTagLiveData(tag)
    val observer = Observer<List<WorkInfo>> { trySend(it).isSuccess }
    work.observeForever(observer)
    awaitClose { work.removeObserver(observer) }
}
