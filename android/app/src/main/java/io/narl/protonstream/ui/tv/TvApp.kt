package io.narl.protonstream.ui.tv

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.focusGroup
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.focusRestorer
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.tv.material3.Icon
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.NavigationDrawer
import androidx.tv.material3.NavigationDrawerItem
import androidx.tv.material3.Text
import androidx.work.WorkManager
import io.narl.protonstream.TvRequest
import io.narl.protonstream.playback.LibmpvHost
import io.narl.protonstream.playback.PlayerScreen
import io.narl.protonstream.ui.AccountActions
import io.narl.protonstream.ui.ActivityBanner
import io.narl.protonstream.ui.AppViewModel
import io.narl.protonstream.ui.DownloadsScreen
import io.narl.protonstream.ui.FocusReturn
import io.narl.protonstream.ui.HistoryScreen
import io.narl.protonstream.ui.LocalFocusReturn
import io.narl.protonstream.ui.SettingsScreen
import io.narl.protonstream.ui.SharesScreen
import io.narl.protonstream.ui.label
import io.narl.protonstream.ui.tryFocus
import kotlinx.coroutines.delay
import uniffi.pstr_android.BrowseFacet
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord

/** Where the drawer goes. Search has a place of its own: there is no bar to put an icon in. */
private enum class Place(val label: String, val icon: ImageVector) {
    Home("Home", Icons.Default.Home),
    Search("Search", Icons.Default.Search),
    History("History", Icons.Default.History),
    Downloads("Downloads", Icons.Default.Download),
    Shares("Shares", Icons.Default.Share),
    Settings("Settings", Icons.Default.Settings),
}

/**
 * The app on a television: the same library, account and player as on a phone,
 * laid out for a screen across a room and a remote with five keys.
 *
 * A drawer down the left edge, closed to its icons until the focus moves into
 * it. Home, a title's page, Search and Browse are the television's own; History,
 * Downloads, Shares and Settings are the phone's pages, which work with a D-pad
 * and are visited rarely enough not to need more.
 *
 * The player covers everything while it plays, and nothing is composed under
 * it: a television has no mini player, and a backdrop drawn behind the video is
 * work the set does for nothing. What the pages need to come back to — which
 * title, how far down the rows — is kept here instead.
 */
@Composable
fun TvApp(
    playerHost: LibmpvHost? = null,
    shareLink: String? = null,
    onShareLinkTaken: () -> Unit = {},
    request: TvRequest? = null,
    onRequestTaken: () -> Unit = {},
    model: AppViewModel = viewModel(
        factory = AppViewModel.Factory(
            LocalContext.current,
            WorkManager.getInstance(LocalContext.current),
        ),
    ),
) {
    val state by model.state.collectAsState()
    val snackbars = remember { SnackbarHostState() }
    var place by rememberSaveable { mutableStateOf(Place.Home) }
    var selectedTitleKey by rememberSaveable { mutableStateOf<String?>(null) }
    var playingTitleKey by rememberSaveable { mutableStateOf<String?>(null) }
    var playingIndex by rememberSaveable { mutableIntStateOf(0) }
    var heroKey by rememberSaveable { mutableStateOf<String?>(null) }
    var addingShare by remember { mutableStateOf(false) }
    // The remote's Search key: the Search page, listening.
    var listening by remember { mutableStateOf(false) }
    // A search from the assistant: Search shows what it found, focused.
    var revealing by remember { mutableStateOf(false) }
    var browseKind by rememberSaveable { mutableStateOf<String?>(null) }
    var browseName by rememberSaveable { mutableStateOf("") }
    val browsing = when (browseKind) {
        "genre" -> BrowseFacet.Genre(browseName)
        "studio" -> BrowseFacet.Studio(browseName)
        "person" -> BrowseFacet.Person(browseName)
        else -> null
    }
    // How far each row and page was scrolled, kept while a title or the player
    // is in front of them.
    val rowStates = remember { mutableMapOf<String, LazyListState>() }
    val rows = { key: String -> rowStates.getOrPut(key) { LazyListState() } }
    val homePage = rememberLazyListState()
    val titlePage = remember(selectedTitleKey) { LazyListState() }

    val selectedTitle = state.titles.firstOrNull { it.key == selectedTitleKey }
    val playing = playingTitleKey?.let { key -> state.titles.firstOrNull { it.key == key } }
    val play = { title: TitleRecord, index: Int ->
        playingTitleKey = title.key
        playingIndex = index
    }
    val close = {
        playingTitleKey = null
        model.playerClosed()
    }

    LaunchedEffect(shareLink) {
        if (shareLink != null) {
            place = Place.Shares
            selectedTitleKey = null
            browseKind = null
        }
    }
    // A Watch Next card or the assistant, once the library is there to answer it.
    LaunchedEffect(request, state.loading, state.titles) {
        val asked = request ?: return@LaunchedEffect
        if (state.loading) return@LaunchedEffect
        val search = { query: String ->
            place = Place.Search
            selectedTitleKey = null
            browseKind = null
            model.search(query)
            revealing = true
        }
        when (asked) {
            is TvRequest.Play -> {
                val title = state.titles.firstOrNull { it.key == asked.titleKey }
                val index = title?.seasons?.flatMap(SeasonRecord::episodes)?.indexOfFirst { it.linkId == asked.linkId } ?: -1
                if (title != null && index >= 0) {
                    play(title, index)
                } else {
                    model.reportError(IllegalStateException("That episode is no longer in the library"))
                }
            }
            is TvRequest.Search -> search(asked.query)
            is TvRequest.PlayFromSearch ->
                playFromSearch(state.titles, asked.query)?.let { (title, index) -> play(title, index) }
                    ?: search(asked.query)
        }
        onRequestTaken()
    }
    LaunchedEffect(state.message) {
        state.message?.let {
            val undoable = state.undo != null
            val result = snackbars.showSnackbar(
                it,
                actionLabel = if (undoable) "Undo" else null,
                duration = if (undoable) SnackbarDuration.Long else SnackbarDuration.Short,
            )
            if (result == SnackbarResult.ActionPerformed) model.undo() else model.dismissMessage()
        }
    }
    // A search narrows the library's arrangement, which Home's rows read as
    // well: leaving Search gives Home the whole library back.
    LaunchedEffect(place) {
        if (place != Place.Search && state.query.isNotEmpty()) model.search("")
    }
    // Closing a title disposes the card it was opened from, and the focus with
    // it; the card takes it back (`returnsFocus`).
    var returnFocusTo by remember { mutableStateOf<String?>(null) }
    var lastTitle by remember { mutableStateOf(selectedTitleKey) }
    LaunchedEffect(selectedTitleKey) {
        if (selectedTitleKey == null) returnFocusTo = lastTitle
        lastTitle = selectedTitleKey
    }

    val homeActions = HomeActions(
        onPlay = play,
        onOpen = { selectedTitleKey = it.key },
        onSetOnList = model::setOnList,
        onSetTitleWatched = model::setTitleWatched,
        onForgetPosition = model::forgetPosition,
        onFilter = model::filter,
        onAddShare = {
            place = Place.Shares
            addingShare = true
        },
    )

    // The innermost thing open goes first: a title, then a browse page, then
    // back to Home from another place. Home's own Back leaves the app.
    BackHandler(enabled = place != Place.Home && selectedTitle == null && browsing == null) { place = Place.Home }
    BackHandler(enabled = browsing != null && selectedTitle == null) { browseKind = null }
    BackHandler(enabled = selectedTitle != null) { selectedTitleKey = null }

    TvTheme {
        Box(
            Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background)
                .onPreviewKeyEvent { event ->
                    if (event.type != KeyEventType.KeyDown || event.key != Key.Search) return@onPreviewKeyEvent false
                    place = Place.Search
                    selectedTitleKey = null
                    browseKind = null
                    listening = true
                    true
                },
        ) {
            if (playing == null) {
                // The page, not the menu, holds the focus when something opens:
                // the app just started (the window hands its first focus to the
                // menu, the first thing in it), a page was picked from the menu,
                // a title closed whose card has scrolled away. A remote with
                // nothing focused does nothing at all.
                var pageFocus by remember { mutableStateOf(false) }
                val content = remember { FocusRequester() }
                LaunchedEffect(state.loading, place, selectedTitleKey, browseKind) {
                    delay(FOCUS_SETTLE_MS)
                    if (!pageFocus) content.tryFocus()
                }
                NavigationDrawer(
                    drawerContent = {
                        // Into the menu lands on the page that is open, not on
                        // whichever entry happens to sit level with the card left.
                        val current = remember { FocusRequester() }
                        Column(
                            Modifier
                                .fillMaxHeight()
                                .padding(vertical = TV_TOP, horizontal = 12.dp)
                                .focusRestorer(current)
                                .focusGroup()
                                .selectableGroup(),
                            verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
                        ) {
                            Place.entries.forEach { item ->
                                NavigationDrawerItem(
                                    modifier = if (item == place) Modifier.focusRequester(current) else Modifier,
                                    selected = place == item && selectedTitle == null && browsing == null,
                                    onClick = {
                                        place = item
                                        selectedTitleKey = null
                                        browseKind = null
                                    },
                                    leadingContent = { Icon(item.icon, contentDescription = null) },
                                ) { Text(item.label) }
                            }
                        }
                    },
                ) {
                    CompositionLocalProvider(LocalFocusReturn provides FocusReturn(returnFocusTo) { returnFocusTo = null }) {
                        Box(
                            Modifier
                                .fillMaxSize()
                                .onFocusChanged { pageFocus = it.hasFocus }
                                .focusRequester(content)
                                .focusGroup(),
                        ) {
                            Page(
                                selectedTitle = selectedTitle,
                                browsing = browsing,
                                place = place,
                                model = model,
                                playerReady = playerHost != null,
                                homeActions = homeActions,
                                rows = rows,
                                homePage = homePage,
                                titlePage = titlePage,
                                heroKey = heroKey,
                                onHero = { heroKey = it },
                                onBrowse = { facet ->
                                    browseKind = when (facet) {
                                        is BrowseFacet.Genre -> "genre"
                                        is BrowseFacet.Studio -> "studio"
                                        is BrowseFacet.Person -> "person"
                                    }
                                    browseName = facet.label
                                },
                                shareLink = shareLink ?: "".takeIf { addingShare },
                                onShareLinkTaken = {
                                    addingShare = false
                                    onShareLinkTaken()
                                },
                                onShareAdded = { place = Place.Home },
                                listening = listening,
                                onListened = { listening = false },
                                revealing = revealing,
                                onRevealed = { revealing = false },
                            )
                        }
                    }
                }
                ActivityBanner(
                    state.activity,
                    onStop = model::stopCrawl.takeIf { state.crawling },
                    modifier = Modifier.align(Alignment.BottomCenter).padding(horizontal = TV_SIDE, vertical = TV_TOP),
                )
            }
            SnackbarHost(snackbars, Modifier.align(Alignment.BottomCenter).padding(horizontal = TV_SIDE, vertical = TV_TOP))
        }
    }

    playing?.let { title ->
        val episodes = remember(title) { title.seasons.flatMap(SeasonRecord::episodes) }
        PlayerScreen(
            title = title,
            episodes = episodes,
            index = playingIndex.coerceIn(0, (episodes.size - 1).coerceAtLeast(0)),
            host = playerHost,
            minimized = false,
            onIndexChange = { playingIndex = it },
            onSaveProgress = model::saveProgress,
            // No mini player on a television: leaving the video stops it.
            onMinimize = close,
            onClose = close,
        )
    }
}

/** The page in front: a title, a browse page, or the place the drawer is on. */
@Composable
private fun Page(
    selectedTitle: TitleRecord?,
    browsing: BrowseFacet?,
    place: Place,
    model: AppViewModel,
    playerReady: Boolean,
    homeActions: HomeActions,
    rows: (String) -> LazyListState,
    homePage: LazyListState,
    titlePage: LazyListState,
    heroKey: String?,
    onHero: (String) -> Unit,
    onBrowse: (BrowseFacet) -> Unit,
    shareLink: String?,
    onShareLinkTaken: () -> Unit,
    onShareAdded: () -> Unit,
    listening: Boolean,
    onListened: () -> Unit,
    revealing: Boolean,
    onRevealed: () -> Unit,
) {
    val state by model.state.collectAsState()
    // The phone's pages, which draw their own bars, inside the overscan margin.
    val phonePage = Modifier.padding(end = TV_SIDE, top = TV_TOP)
    val phonePadding = PaddingValues(bottom = TV_TOP)
    when {
        selectedTitle != null -> {
            val similar by produceState(emptyList<String>(), selectedTitle.key, state.titles) {
                value = model.moreLike(selectedTitle.key)
            }
            TvTitle(
                selectedTitle,
                playerReady,
                TitleActions(
                    onPlay = { index -> homeActions.onPlay(selectedTitle, index) },
                    onSetOnList = model::setOnList,
                    onSetTitleWatched = model::setTitleWatched,
                    onMarkEpisodes = model::markEpisodes,
                    onOpenTitle = homeActions.onOpen,
                ),
                franchise = selectedTitle.franchise.mapNotNull { key -> state.titles.firstOrNull { it.key == key } },
                moreLike = similar.mapNotNull { key -> state.titles.firstOrNull { it.key == key } },
                page = titlePage,
            )
        }
        browsing != null -> {
            val browsed by produceState(emptyList<String>(), browsing, state.titles) {
                value = model.browse(browsing)
            }
            TvBrowse(browsing, browsed.mapNotNull { key -> state.titles.firstOrNull { it.key == key } }, homeActions)
        }
        else -> when (place) {
            Place.Home -> TvHome(state, homeActions, rows, homePage, heroKey, onHero)
            Place.Search -> TvSearch(
                state,
                model::search,
                homeActions,
                onBrowse,
                model::rememberSearch,
                listen = listening,
                onListening = onListened,
                reveal = revealing,
                onRevealed = onRevealed,
            )
            Place.History -> Box(phonePage) {
                HistoryScreen(state, homeActions.onPlay, homeActions.onOpen, model::removeFromHistory, phonePadding)
            }
            Place.Downloads -> Box(phonePage) {
                DownloadsScreen(
                    state,
                    model::removeOffline,
                    model::pauseDownload,
                    model::resumeDownload,
                    model::deletePartial,
                    phonePadding,
                    onPlay = homeActions.onPlay,
                    onOpen = homeActions.onOpen,
                )
            }
            Place.Shares -> Box(phonePage) {
                SharesScreen(
                    state.shares,
                    { name, url, password ->
                        // The first share is the library: watch it fill rather
                        // than stay on a form that has done its job.
                        val first = state.titles.isEmpty()
                        model.addShare(name, url, password).also { refusal ->
                            if (refusal == null && first) onShareAdded()
                        }
                    },
                    model::repairShare,
                    model::refreshShare,
                    model::removeShare,
                    shareLink,
                    onShareLinkTaken,
                    phonePadding,
                    account = state.account,
                    accountActions = AccountActions(
                        signIn = model::signIn,
                        submitSecondFactor = model::submitSecondFactor,
                        submitMailboxPassword = model::submitMailboxPassword,
                        cancelSignIn = model::cancelSignIn,
                        dismissVerification = model::dismissVerification,
                        signOut = model::signOut,
                        syncNow = model::syncNow,
                        addFolder = model::addAccountFolder,
                        onError = model::reportError,
                    ),
                    onRename = model::renameShare,
                    startHandoff = model::startShareHandoff,
                )
            }
            Place.Settings -> Box(phonePage) {
                SettingsScreen(
                    state,
                    model::saveMetadataSettings,
                    { model.matchTitles(force = true) },
                    model::clearBlockCache,
                    model::removeAllOffline,
                    phonePadding,
                    onNamesChanged = model::reloadAfterMetadataChange,
                    onWatchNextChanged = model::setWatchNext,
                )
            }
        }
    }
}

/** Long enough for a page's own focus (a title's Play, a card coming back) to land first. */
private const val FOCUS_SETTLE_MS = 150L
