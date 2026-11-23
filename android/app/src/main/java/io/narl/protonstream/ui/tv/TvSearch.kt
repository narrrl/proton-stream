package io.narl.protonstream.ui.tv

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.OutlinedTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.tv.material3.ExperimentalTvMaterial3Api
import androidx.tv.material3.FilterChip
import androidx.tv.material3.Icon
import androidx.tv.material3.ListItem
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import io.narl.protonstream.ui.AppUiState
import io.narl.protonstream.ui.isEmpty
import io.narl.protonstream.ui.label
import io.narl.protonstream.ui.tryFocus
import uniffi.pstr_android.BrowseFacet
import uniffi.pstr_android.FacetCountRecord
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord

/**
 * Search, on a television: the field at the top, and what it found under it —
 * people, studios and genres, episodes, then titles.
 *
 * The field takes the focus when the page opens, and OK on it brings up the
 * television's keyboard (or its voice input). Search on the keyboard moves the
 * focus down to what was found.
 */
@OptIn(ExperimentalTvMaterial3Api::class)
@Composable
internal fun TvSearch(
    state: AppUiState,
    onSearch: (String) -> Unit,
    actions: HomeActions,
    onBrowse: (BrowseFacet) -> Unit,
    onRemember: () -> Unit,
) {
    val byKey = remember(state.titles) { state.titles.associateBy { it.key } }
    val searched = state.query.isNotBlank()
    val tiles = remember(state.titles, state.arrangement, searched) {
        if (!searched) emptyList() else state.arrangement?.tiles?.mapNotNull { tile -> byKey[tile.key]?.let { it to 0 } }.orEmpty()
    }
    var menuFor by remember { mutableStateOf<TitleRecord?>(null) }
    val field = remember { FocusRequester() }
    val focus = LocalFocusManager.current
    LaunchedEffect(Unit) { field.tryFocus() }
    // A search that led somewhere is one worth offering again.
    val open = { title: TitleRecord ->
        onRemember()
        actions.onOpen(title)
    }

    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(top = TV_TOP, bottom = TV_TOP),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item(key = "field") {
            OutlinedTextField(
                value = state.query,
                onValueChange = onSearch,
                singleLine = true,
                placeholder = { androidx.compose.material3.Text("Titles, episodes, people, genres") },
                leadingIcon = { androidx.compose.material3.Icon(Icons.Default.Search, contentDescription = null) },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { focus.moveFocus(FocusDirection.Down) }),
                modifier = Modifier
                    .padding(horizontal = TV_SIDE)
                    .fillMaxWidth(SEARCH_FIELD_WIDTH)
                    .focusRequester(field),
            )
        }
        if (!searched) {
            if (state.recentSearches.isNotEmpty()) {
                item(key = "recent") {
                    Column(Modifier.padding(horizontal = TV_SIDE)) {
                        Text("Recent searches", style = MaterialTheme.typography.titleMedium)
                        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            state.recentSearches.forEach { recent ->
                                FilterChip(selected = false, onClick = { onSearch(recent) }) { Text(recent) }
                            }
                        }
                    }
                }
            }
            return@LazyColumn
        }
        val extras = state.searchExtras
        if (tiles.isEmpty() && extras?.isEmpty() != false) {
            item(key = "nothing") {
                Text(
                    "Nothing in the library is called “${state.query.trim()}”.",
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = TV_SIDE),
                )
            }
            return@LazyColumn
        }
        extras?.let {
            facets("People and studios", it.people, onBrowse)
            facets("Genres", it.genres, onBrowse)
        }
        extras?.episodes?.takeIf { it.isNotEmpty() }?.let { hits ->
            item(key = "episodes-heading") {
                Text("Episodes", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(horizontal = TV_SIDE))
            }
            items(hits, key = { "episode-${it.titleKey}/${it.linkId}" }) { hit ->
                val title = byKey[hit.titleKey] ?: return@items
                val index = title.seasons.flatMap(SeasonRecord::episodes).indexOfFirst { it.linkId == hit.linkId }
                if (index < 0) return@items
                ListItem(
                    selected = false,
                    onClick = {
                        onRemember()
                        actions.onPlay(title, index)
                    },
                    headlineContent = { Text(hit.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    supportingContent = {
                        Text("${title.displayName} · ${hit.label}", maxLines = 1, overflow = TextOverflow.Ellipsis)
                    },
                    leadingContent = { Icon(Icons.Default.PlayArrow, contentDescription = null) },
                    modifier = Modifier.padding(horizontal = TV_SIDE).fillMaxWidth(SEARCH_FIELD_WIDTH),
                )
            }
        }
        if (tiles.isNotEmpty()) {
            item(key = "titles") {
                Column {
                    Text("Titles", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(horizontal = TV_SIDE))
                    PosterGrid(tiles, onOpen = open, onMenu = { menuFor = it })
                }
            }
        }
    }
    menuFor?.let { TitleMenu(it, actions) { menuFor = null } }
}

/** A row of the people, studios or genres a search named, each opening its page. */
@OptIn(ExperimentalTvMaterial3Api::class)
private fun androidx.compose.foundation.lazy.LazyListScope.facets(
    heading: String,
    found: List<FacetCountRecord>,
    onBrowse: (BrowseFacet) -> Unit,
) {
    if (found.isEmpty()) return
    item(key = "facets-$heading") {
        Column(Modifier.padding(horizontal = TV_SIDE)) {
            Text(heading, style = MaterialTheme.typography.titleMedium)
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                found.take(FACETS_SHOWN).forEach { facet ->
                    FilterChip(selected = false, onClick = { onBrowse(facet.facet) }) {
                        Text("${facet.facet.label} · ${facet.count}")
                    }
                }
            }
        }
    }
}

/**
 * Every title sharing a genre, a studio or a person: what a chip on a title
 * page or in a search opens.
 */
@Composable
internal fun TvBrowse(facet: BrowseFacet, titles: List<TitleRecord>, actions: HomeActions) {
    var menuFor by remember { mutableStateOf<TitleRecord?>(null) }
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(top = TV_TOP, bottom = TV_TOP),
    ) {
        item(key = "heading") {
            Text(
                facet.label,
                style = MaterialTheme.typography.headlineMedium,
                modifier = Modifier.padding(horizontal = TV_SIDE),
            )
        }
        item(key = "titles") {
            PosterGrid(titles.map { it to 0 }, onOpen = actions.onOpen, onMenu = { menuFor = it })
        }
    }
    menuFor?.let { TitleMenu(it, actions) { menuFor = null } }
}

/** How far across the field and the episode hits run: a line, not the screen. */
private const val SEARCH_FIELD_WIDTH = 0.6f

/** More than this many chips run off the side of a television, out of reach. */
private const val FACETS_SHOWN = 6
