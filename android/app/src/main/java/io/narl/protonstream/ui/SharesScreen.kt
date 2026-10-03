package io.narl.protonstream.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import androidx.compose.ui.unit.dp
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.QuietButton
import io.narl.protonstream.ui.theme.TonalButton
import uniffi.pstr_android.ShareRecord

@Composable
internal fun SharesScreen(
    shares: List<ShareRecord>,
    onAdd: (String, String, String?) -> Unit,
    onRepair: (String, String, String?) -> Unit,
    onRefresh: (String) -> Unit,
    onRemove: (String) -> Unit,
    padding: PaddingValues,
) {
    var showAdd by remember { mutableStateOf(false) }
    var repairing by remember { mutableStateOf<ShareRecord?>(null) }
    Column(Modifier.fillMaxSize().padding(padding).padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("Proton Drive public links", style = MaterialTheme.typography.titleLarge)
            AccentButton(onClick = { showAdd = true }) { Text("Add share") }
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 16.dp)) {
            items(shares, key = { it.id }) { share ->
                Card(Modifier.fillMaxWidth()) {
                    Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(share.name, fontWeight = FontWeight.SemiBold)
                            Text(if (share.hasCustomPassword) "Custom password stored securely" else "Public link")
                        }
                        // One share, not the library: a link that has just had
                        // files added to it should not cost a walk of every
                        // other one, and a link that has expired should not stop
                        // the ones that still work from being refreshed.
                        IconButton(onClick = { onRefresh(share.id) }) {
                            Icon(Icons.Default.Refresh, contentDescription = "Refresh this share")
                        }
                        // Not a remove: a share whose secret has become
                        // unreadable cannot be removed either, since removal
                        // deletes a secret the store can no longer touch.
                        QuietButton(onClick = { repairing = share }) { Text("Re-enter link") }
                        TonalButton(onClick = { onRemove(share.id) }) { Text("Remove") }
                    }
                }
            }
        }
    }
    if (showAdd) AddShareDialog(onDismiss = { showAdd = false }, onAdd = onAdd)
    repairing?.let { share ->
        RepairShareDialog(
            share = share,
            onDismiss = { repairing = null },
            onRepair = { url, password -> onRepair(share.id, url, password) },
        )
    }
}

/**
 * Re-enter the link for a share the app can no longer decrypt its secret for.
 *
 * It has to be the *same* link — a different token is a different share, and
 * repointing this one at it would leave every catalog row and every offline file
 * describing something that is not there. Rust enforces that; this only says so.
 */
@Composable
private fun RepairShareDialog(
    share: ShareRecord,
    onDismiss: () -> Unit,
    onRepair: (String, String?) -> Unit,
) {
    var url by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Re-enter link for ${share.name}") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    "The stored credentials for this share cannot be read — usually after a " +
                        "screen-lock change or a device restore. Entering the same link again " +
                        "restores access without losing the library or anything downloaded.",
                    style = MaterialTheme.typography.bodySmall,
                )
                OutlinedTextField(
                    url,
                    { url = it },
                    label = { Text("Public share URL") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
                OutlinedTextField(
                    password,
                    { password = it },
                    label = { Text("Custom password (optional)") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
            }
        },
        confirmButton = {
            AccentButton(
                onClick = { onRepair(url.trim(), password); onDismiss() },
                enabled = url.isNotBlank(),
            ) { Text("Restore") }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun AddShareDialog(onDismiss: () -> Unit, onAdd: (String, String, String?) -> Unit) {
    var name by remember { mutableStateOf("") }
    var url by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Add Proton Drive share") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(name, { name = it }, label = { Text("Library name") }, singleLine = true)
                OutlinedTextField(
                    url,
                    { url = it },
                    label = { Text("Public share URL") },
                    singleLine = true,
                    // The URL fragment is a secret. Password input disables
                    // IME learning/suggestions while normal long-press paste
                    // remains available.
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
                OutlinedTextField(
                    password,
                    { password = it },
                    label = { Text("Custom password (optional)") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                )
            }
        },
        confirmButton = {
            AccentButton(
                onClick = { onAdd(name.trim(), url.trim(), password); onDismiss() },
                enabled = name.isNotBlank() && url.isNotBlank(),
            ) { Text("Add") }
        },
        dismissButton = { TonalButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
