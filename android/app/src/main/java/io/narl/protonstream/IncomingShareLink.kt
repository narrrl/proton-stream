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

/**
 * A `pstr://` invite (`pstr_core::invite`): one link carrying one share or a
 * whole library, passwords included. Its payload is base64url, so the same
 * stop characters end it.
 */
private val INVITE = Regex("""pstr://add[^\s"'<>()\[\]]*""", RegexOption.IGNORE_CASE)

/**
 * The share link or invite an `ACTION_VIEW` URL or an `ACTION_SEND` text
 * carries, if any. In a message holding both, the invite wins: it is the
 * newer, deliberate form.
 */
internal fun shareLinkFrom(action: String?, url: String?, text: CharSequence?): String? = when (action) {
    Intent.ACTION_VIEW -> url?.let { SHARE_LINK.matchEntire(it)?.value ?: INVITE.matchEntire(it)?.value }
    Intent.ACTION_SEND -> text?.let { INVITE.find(it)?.value ?: SHARE_LINK.find(it)?.value }
    else -> null
}

/** Whether an incoming link is an invite, to confirm, rather than one share link for the form. */
internal fun isInviteLink(text: String): Boolean = text.trimStart().startsWith("pstr://", ignoreCase = true)

internal fun Intent.shareLink(): String? =
    shareLinkFrom(action, dataString, getCharSequenceExtra(Intent.EXTRA_TEXT))
