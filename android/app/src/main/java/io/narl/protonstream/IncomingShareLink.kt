package io.narl.protonstream

import android.content.Intent

/**
 * A Proton Drive public link, wherever in a piece of text it sits.
 *
 * The fragment is kept: it is the share's secret, and a link without it cannot
 * be opened. Matching stops at whitespace and at the quotes and brackets a
 * message app wraps a link in, which never occur in a link itself.
 */
private val SHARE_LINK = Regex("""https://drive\.proton\.me/urls/[^\s"'<>()\[\]]+""")

/** The share link an `ACTION_VIEW` URL or an `ACTION_SEND` text carries, if any. */
internal fun shareLinkFrom(action: String?, url: String?, text: CharSequence?): String? = when (action) {
    Intent.ACTION_VIEW -> url?.let { SHARE_LINK.matchEntire(it)?.value }
    Intent.ACTION_SEND -> text?.let { SHARE_LINK.find(it)?.value }
    else -> null
}

internal fun Intent.shareLink(): String? =
    shareLinkFrom(action, dataString, getCharSequenceExtra(Intent.EXTRA_TEXT))
