package io.narl.protonstream.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.AlertDialog
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
import io.narl.protonstream.ui.theme.TonalButton
import uniffi.pstr_android.ShareRecord
import androidx.compose.foundation.layout.Box
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Key
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.FolderShared
import androidx.compose.ui.graphics.Color

@Composable
internal fun SharesScreen(
    shares: List<ShareRecord>,
    onAdd: (String, String, String?) -> Unit,
    onRepair: (String, String, String?) -> Unit,
    onRefresh: (String) -> Unit,
    onRemove: (String) -> Unit,
    incomingLink: String?,
    onIncomingLinkTaken: () -> Unit,
    padding: PaddingValues,
) {
    var showAdd by remember { mutableStateOf(false) }
    var repairing by remember { mutableStateOf<ShareRecord?>(null) }
    var removing by remember { mutableStateOf<ShareRecord?>(null) }
    Box(Modifier.fillMaxSize().padding(padding)) {
        if (shares.isEmpty()) {
            EmptyState("No shares yet", "Add a Proton Drive public link to build your library.")
        } else {
            LazyColumn(contentPadding = PaddingValues(top = 8.dp, bottom = 96.dp)) {
                item(key = "heading") {
                    Text(
                        "Proton Drive public links",
                        style = MaterialTheme.typography.titleMedium,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
                items(shares, key = { it.id }) { share ->
                    ShareRow(
                        share = share,
                        onRefresh = { onRefresh(share.id) },
                        onRepair = { repairing = share },
                        onRemove = { removing = share },
                    )
                }
            }
        }
        // Where a thumb is, and the one thing this page is for. The share
        // sheet and a tapped link land in the same form.
        ExtendedFloatingActionButton(
            onClick = { showAdd = true },
            icon = { Icon(Icons.Default.Add, contentDescription = null) },
            text = { Text("Add share") },
            containerColor = MaterialTheme.colorScheme.primary,
            contentColor = MaterialTheme.colorScheme.onPrimary,
            modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp),
        )
    }
    if (showAdd || incomingLink != null) {
        AddShareDialog(
            initialUrl = incomingLink.orEmpty(),
            onDismiss = { showAdd = false; onIncomingLinkTaken() },
            onAdd = onAdd,
        )
    }
    repairing?.let { share ->
        RepairShareDialog(
            share = share,
            onDismiss = { repairing = null },
            onRepair = { url, password -> onRepair(share.id, url, password) },
        )
    }
    removing?.let { share ->
        // Asked, because it cannot be undone: the link's secret, its offline
        // files and its watch positions all go (`Catalog::remove_share`).
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Remove ${share.name}?") },
            text = {
                Text(
                    "Its titles leave the library, and its offline episodes and watch " +
                        "progress are deleted. Adding the link again starts it fresh.",
                )
            },
            confirmButton = {
                AccentButton(onClick = { removing = null; onRemove(share.id) }) { Text("Remove") }
            },
            dismissButton = { TonalButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
}

/**
 * One share: its name, how it is unlocked, and its actions behind a menu.
 *
 * Three differently styled controls beside the name squeezed it into a column
 * a word wide; one icon leaves the row its width.
 */
@Composable
private fun ShareRow(
    share: ShareRecord,
    onRefresh: () -> Unit,
    onRepair: () -> Unit,
    onRemove: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    ListItem(
        headlineContent = { Text(share.name, fontWeight = FontWeight.SemiBold) },
        supportingContent = {
            Text(if (share.hasCustomPassword) "Link and custom password, stored securely" else "Public link")
        },
        leadingContent = {
            Icon(
                Icons.Default.FolderShared,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.primary,
            )
        },
        trailingContent = {
            Box {
                IconButton(onClick = { menu = true }) {
                    Icon(Icons.Default.MoreVert, contentDescription = "Actions for ${share.name}")
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    // One share, not the library: a link that has just had files
                    // added should not cost a walk of every other one.
                    DropdownMenuItem(
                        text = { Text("Refresh") },
                        leadingIcon = { Icon(Icons.Default.Refresh, contentDescription = null) },
                        onClick = { menu = false; onRefresh() },
                    )
                    // Not a remove: a share whose secret has become unreadable
                    // cannot be removed either, since removal deletes a secret
                    // the store can no longer touch.
                    DropdownMenuItem(
                        text = { Text("Re-enter link") },
                        leadingIcon = { Icon(Icons.Default.Key, contentDescription = null) },
                        onClick = { menu = false; onRepair() },
                    )
                    DropdownMenuItem(
                        text = { Text("Remove") },
                        leadingIcon = { Icon(Icons.Default.Delete, contentDescription = null) },
                        onClick = { menu = false; onRemove() },
                    )
                }
            }
        },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
    )
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
private fun AddShareDialog(
    initialUrl: String,
    onDismiss: () -> Unit,
    onAdd: (String, String, String?) -> Unit,
) {
    var name by remember { mutableStateOf("") }
    var url by remember(initialUrl) { mutableStateOf(initialUrl) }
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
