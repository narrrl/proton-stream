package io.narl.protonstream

import android.app.SearchManager
import android.content.Context
import android.content.Intent

/**
 * What the television asked the app to do from outside it: play an episode
 * from the home screen's Watch Next row, or answer a voice search.
 */
sealed interface TvRequest {
    /** A Watch Next card: this episode of this title, from where it stopped. */
    data class Play(val titleKey: String, val linkId: String) : TvRequest

    /** "Search for … on Proton Stream": show what the library has. */
    data class Search(val query: String) : TvRequest

    /**
     * "Play … on Proton Stream": play a title that is called that, or show
     * what the search found when nothing or several are. A blank query is
     * "play something", which is whatever Continue watching offers first.
     */
    data class PlayFromSearch(val query: String) : TvRequest
}

/** The Watch Next card's intent, which [tvRequestFrom] reads back. */
internal const val ACTION_PLAY_EPISODE = "io.narl.protonstream.action.PLAY_EPISODE"
internal const val EXTRA_TITLE_KEY = "io.narl.protonstream.extra.TITLE_KEY"
internal const val EXTRA_LINK_ID = "io.narl.protonstream.extra.LINK_ID"

/** What an assistant sends for "search for … in <app>", besides the platform's own SEARCH. */
internal const val ACTION_ASSISTANT_SEARCH = "com.google.android.gms.actions.SEARCH_ACTION"

internal fun playEpisodeIntent(context: Context, titleKey: String, linkId: String): Intent =
    Intent(context, MainActivity::class.java)
        .setAction(ACTION_PLAY_EPISODE)
        .putExtra(EXTRA_TITLE_KEY, titleKey)
        .putExtra(EXTRA_LINK_ID, linkId)

/** The request an intent carries, if it carries one. */
internal fun tvRequestFrom(action: String?, extra: (String) -> String?): TvRequest? = when (action) {
    ACTION_PLAY_EPISODE -> {
        val title = extra(EXTRA_TITLE_KEY)?.takeIf { it.isNotBlank() }
        val link = extra(EXTRA_LINK_ID)?.takeIf { it.isNotBlank() }
        if (title != null && link != null) TvRequest.Play(title, link) else null
    }
    Intent.ACTION_SEARCH, ACTION_ASSISTANT_SEARCH ->
        extra(SearchManager.QUERY)?.trim()?.takeIf { it.isNotEmpty() }?.let(TvRequest::Search)
    android.provider.MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH ->
        TvRequest.PlayFromSearch(extra(SearchManager.QUERY)?.trim().orEmpty())
    else -> null
}

internal fun Intent.tvRequest(): TvRequest? = tvRequestFrom(action) { getStringExtra(it) }
