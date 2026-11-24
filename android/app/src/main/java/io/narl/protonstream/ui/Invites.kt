package io.narl.protonstream.ui

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.PersistableBundle
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy

import io.narl.protonstream.native.reason
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.TonalButton
import kotlinx.coroutines.launch
import uniffi.pstr_android.InviteRecord
import uniffi.pstr_android.InvitedShareRecord
import uniffi.pstr_android.qrCode

/**
 * A `pstr://` invite that arrived: what it adds, ticked, and Add.
 *
 * Shares the library already has are shown, unticked and disabled, so a
 * re-sent invite says why it adds less than it lists.
 */
@Composable
internal fun IncomingInviteDialog(
    link: String,
    decode: suspend (String) -> Result<List<InvitedShareRecord>>,
    onAdd: suspend (List<InvitedShareRecord>) -> String?,
    onDismiss: () -> Unit,
) {
    val decoded by produceState<Result<List<InvitedShareRecord>>?>(null, link) { value = decode(link) }
    val shares = decoded?.getOrNull()
    val ticked = remember(shares) {
        mutableStateListOf<Boolean>().apply { shares?.forEach { add(!it.alreadyAdded) } }
    }
    var adding by remember { mutableStateOf(false) }
    var refusal by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val chosen = shares.orEmpty().filterIndexed { index, share -> !share.alreadyAdded && ticked.getOrElse(index) { false } }

    AlertDialog(
        onDismissRequest = { if (!adding) onDismiss() },
        // The invite holds passwords; keep it out of screenshots and recents.
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = {
            Text(
                when (val count = shares?.size ?: 0) {
                    0, 1 -> "Add a shared library?"
                    else -> "Add $count shared libraries?"
                },
            )
        },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                when {
                    decoded == null -> Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(24.dp))
                    }
                    shares == null -> Text(
                        decoded?.exceptionOrNull()?.reason ?: "That invite could not be read.",
                        color = MaterialTheme.colorScheme.error,
                    )
                    else -> {
                        Text(
                            "Someone sent you an invite. Each share opens before it is added.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        LazyColumn(Modifier.heightIn(max = 320.dp)) {
                            items(shares.indices.toList()) { index ->
                                val share = shares[index]
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Checkbox(
                                        checked = !share.alreadyAdded && ticked[index],
                                        onCheckedChange = { ticked[index] = it },
                                        enabled = !share.alreadyAdded && !adding,
                                    )
                                    Column {
                                        Text(share.name.ifBlank { "Shared folder" })
                                        val note = when {
                                            share.alreadyAdded -> "Already in your library"
                                            share.customPassword != null -> "Password included"
                                            else -> null
                                        }
                                        note?.let {
                                            Text(
                                                it,
                                                style = MaterialTheme.typography.bodySmall,
                                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                refusal?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        },
        confirmButton = {
            AccentButton(
                enabled = chosen.isNotEmpty() && !adding,
                onClick = {
                    adding = true
                    scope.launch {
                        val why = onAdd(chosen)
                        adding = false
                        if (why == null) onDismiss() else refusal = why
                    }
                },
            ) {
                if (adding) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                } else {
                    Text(if (chosen.size > 1) "Add ${chosen.size}" else "Add")
                }
            }
        },
        dismissButton = { TonalButton(enabled = !adding, onClick = onDismiss) { Text("Cancel") } },
    )
}

/**
 * An invite to hand out: its QR code, and on a phone, Send and Copy.
 *
 * The link is never printed. It carries every password it shares, so it
 * leaves the app only on purpose, through the share sheet or the clipboard
 * (marked sensitive, so Android 13+ does not preview it).
 */
@Composable
internal fun OutgoingInviteDialog(
    shareIds: List<String>,
    inviteLink: suspend (List<String>) -> Result<InviteRecord>,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val invite by produceState<Result<InviteRecord>?>(null, shareIds) { value = inviteLink(shareIds) }
    val record = invite?.getOrNull()
    val qr = remember(record) { record?.let { runCatching { qrCode(it.link) }.getOrNull() } }
    val television = isTelevision()

    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text(if (shareIds.size == 1) "Invite to this share" else "Invite to your library") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                when {
                    invite == null -> Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(24.dp))
                    }
                    record == null -> Text(
                        invite?.exceptionOrNull()?.reason ?: "No invite could be made.",
                        color = MaterialTheme.colorScheme.error,
                    )
                    else -> {
                        Text(
                            "Scan with a device that has Proton Stream, or send the link.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        qr?.let {
                            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                                QrCode(it, "an invite link", Modifier.size(220.dp))
                            }
                        }
                        Text(
                            "Anyone with this invite can open what it shares, passwords included. " +
                                "Send it only to people you trust.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (record.leftOut > 0u) {
                            Text(
                                "${record.leftOut} folder(s) of your Proton account left out: they have no link to share.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            if (record != null && !television) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TonalButton(onClick = { copyInvite(context, record.link); onDismiss() }) { Text("Copy") }
                    AccentButton(onClick = { sendInvite(context, record.link) }) { Text("Send") }
                }
            } else {
                AccentButton(onClick = onDismiss) { Text("Done") }
            }
        },
        dismissButton = if (record != null && !television) {
            { TonalButton(onClick = onDismiss) { Text("Close") } }
        } else {
            null
        },
    )
}

private fun sendInvite(context: Context, link: String) {
    val send = Intent(Intent.ACTION_SEND).apply {
        type = "text/plain"
        putExtra(Intent.EXTRA_TEXT, "Watch with me on Proton Stream: $link")
    }
    context.startActivity(Intent.createChooser(send, "Send invite").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}

private fun copyInvite(context: Context, link: String) {
    val clip = ClipData.newPlainText("Proton Stream invite", link)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        clip.description.extras = PersistableBundle().apply {
            putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
        }
    }
    context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(clip)
}
