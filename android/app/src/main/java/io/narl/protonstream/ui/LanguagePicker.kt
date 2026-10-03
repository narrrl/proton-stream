package io.narl.protonstream.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.TonalButton

/**
 * The languages offered by name, as the three-letter tag a Matroska file
 * carries.
 *
 * The same short list the desktop names tracks from (`pstr-player`'s
 * `language_name`): what the shares this app is pointed at actually contain.
 * mpv treats the two- and three-letter forms of a language as one, so the tag
 * chosen here also matches a file that says "ja". Anything else is still one
 * typed tag away.
 */
internal val LANGUAGES: List<Pair<String, String>> = listOf(
    "jpn" to "Japanese",
    "eng" to "English",
    "ger" to "German",
    "fre" to "French",
    "spa" to "Spanish",
    "ita" to "Italian",
    "por" to "Portuguese",
    "dut" to "Dutch",
    "rus" to "Russian",
    "pol" to "Polish",
    "swe" to "Swedish",
    "dan" to "Danish",
    "nor" to "Norwegian",
    "fin" to "Finnish",
    "cze" to "Czech",
    "hun" to "Hungarian",
    "tur" to "Turkish",
    "ara" to "Arabic",
    "heb" to "Hebrew",
    "hin" to "Hindi",
    "kor" to "Korean",
    "chi" to "Chinese",
    "tha" to "Thai",
    "vie" to "Vietnamese",
    "ukr" to "Ukrainian",
    "rum" to "Romanian",
    "gre" to "Greek",
    "ind" to "Indonesian",
    "may" to "Malay",
)

/**
 * "Japanese", for a tag in any of the forms a file or the desktop may have
 * stored — "jpn", "ja", "JA-jp" — and the tag itself for one not on the list.
 * Null is no preference.
 */
internal fun languageLabel(tag: String?): String {
    val value = tag?.trim()?.takeIf(String::isNotEmpty) ?: return "No preference"
    return languageOf(value)?.second ?: value
}

/** The listed language a tag names, matching either ISO 639 form. */
private fun languageOf(tag: String): Pair<String, String>? {
    val base = tag.split('-', '_').first().lowercase()
    return LANGUAGES.firstOrNull { (code, _) -> code == base || TWO_LETTER[code] == base || terminology(code) == base }
}

/** ISO 639-2/T, for the handful whose bibliographic code differs. */
private fun terminology(code: String): String? = when (code) {
    "ger" -> "deu"
    "fre" -> "fra"
    "dut" -> "nld"
    "cze" -> "ces"
    "chi" -> "zho"
    "rum" -> "ron"
    "gre" -> "ell"
    "may" -> "msa"
    else -> null
}

/** ISO 639-1, the form a file or the desktop may also have stored. */
private val TWO_LETTER = mapOf(
    "jpn" to "ja", "eng" to "en", "ger" to "de", "fre" to "fr", "spa" to "es", "ita" to "it",
    "por" to "pt", "dut" to "nl", "rus" to "ru", "pol" to "pl", "swe" to "sv", "dan" to "da",
    "nor" to "no", "fin" to "fi", "cze" to "cs", "hun" to "hu", "tur" to "tr", "ara" to "ar",
    "heb" to "he", "hin" to "hi", "kor" to "ko", "chi" to "zh", "tha" to "th", "vie" to "vi",
    "ukr" to "uk", "rum" to "ro", "gre" to "el", "ind" to "id", "may" to "ms",
)

/**
 * A language preference as a row, opening a list to choose from.
 *
 * It was a free-text field asking for "jpn or eng", which is a question only
 * someone who has read a Matroska header can answer.
 */
@Composable
internal fun LanguageSetting(headline: String, value: String?, onChange: (String?) -> Unit) {
    var choosing by remember { mutableStateOf(false) }
    ListItem(
        headlineContent = { Text(headline) },
        supportingContent = { Text(languageLabel(value)) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier.clickable(role = Role.Button) { choosing = true },
    )
    if (choosing) {
        LanguageDialog(
            headline,
            value,
            onDismiss = { choosing = false },
            onChoose = {
                choosing = false
                onChange(it)
            },
        )
    }
}

@Composable
private fun LanguageDialog(headline: String, value: String?, onDismiss: () -> Unit, onChoose: (String?) -> Unit) {
    val current = value?.let(::languageOf)?.first ?: value
    // A tag that is not on the list starts in the field, so it is shown and
    // can be edited rather than silently replaced.
    var typed by remember { mutableStateOf(current?.takeIf { tag -> LANGUAGES.none { it.first == tag } }.orEmpty()) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(headline) },
        text = {
            Column {
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 360.dp)) {
                    item(key = "none") { LanguageOption("No preference", current == null) { onChoose(null) } }
                    items(LANGUAGES, key = { it.first }) { (code, name) ->
                        LanguageOption(name, current == code) { onChoose(code) }
                    }
                }
                HorizontalDivider(Modifier.padding(vertical = 8.dp))
                OutlinedTextField(
                    value = typed,
                    onValueChange = { typed = it },
                    label = { Text("Another tag") },
                    supportingText = { Text("As it appears in the file, such as \"tgl\"") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = {
            AccentButton(enabled = typed.isNotBlank(), onClick = { onChoose(typed.trim().lowercase()) }) {
                Text("Use tag")
            }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun LanguageOption(name: String, selected: Boolean, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(name, style = MaterialTheme.typography.bodyLarge) },
        leadingContent = { RadioButton(selected = selected, onClick = null) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier.clickable(role = Role.RadioButton, onClick = onClick),
    )
}
