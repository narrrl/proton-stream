package io.narl.protonstream.ui.tv

import android.app.Activity
import android.content.Intent
import android.speech.RecognizerIntent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import io.narl.protonstream.ui.continueWatching
import io.narl.protonstream.ui.nextUpIndex
import uniffi.pstr_android.SeasonRecord
import uniffi.pstr_android.TitleRecord

/**
 * What "play … on Proton Stream" plays: the one title called that, at the
 * episode Play on its page would start. A blank query asks for anything, which
 * is the top of Continue watching.
 *
 * Names are compared without case, spaces or punctuation, since an assistant
 * writes down what it heard ("steins gate") rather than how the show spells
 * itself ("Steins;Gate"). Two titles of the same name, or none, are a search
 * instead.
 */
internal fun playFromSearch(titles: List<TitleRecord>, query: String): Pair<TitleRecord, Int>? {
    if (query.isBlank()) return continueWatching(titles).firstOrNull()?.let { it.title to it.index }
    val wanted = spoken(query)
    val title = titles.filter { title ->
        listOfNotNull(title.displayName, title.name, title.canonicalName, title.originalName).any { spoken(it) == wanted }
    }.singleOrNull() ?: return null
    val playlist = title.seasons.flatMap(SeasonRecord::episodes)
    if (playlist.isEmpty()) return null
    return title to nextUpIndex(playlist, title.continueIndex)
}

private fun spoken(name: String): String = name.lowercase().filter(Char::isLetterOrDigit)

/**
 * The system's speech input, for the mic on the Search page; null where the
 * television has no recognizer to offer, and the mic is not drawn.
 */
@Composable
internal fun rememberVoiceInput(onHeard: (String) -> Unit): (() -> Unit)? {
    val context = LocalContext.current
    val available = remember {
        Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).resolveActivity(context.packageManager) != null
    }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode != Activity.RESULT_OK) return@rememberLauncherForActivityResult
        result.data?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)
            ?.firstOrNull { it.isNotBlank() }
            ?.let(onHeard)
    }
    if (!available) return null
    return {
        runCatching {
            launcher.launch(
                Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
                    .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_WEB_SEARCH)
                    .putExtra(RecognizerIntent.EXTRA_PROMPT, "Say a title, a person or a genre"),
            )
        }
    }
}
