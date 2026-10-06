package io.narl.protonstream.ui

import android.annotation.SuppressLint
import android.os.Handler
import android.os.Looper
import android.webkit.JavascriptInterface
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.AccountCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Computer
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.FolderShared
import androidx.compose.material.icons.filled.InsertDriveFile
import androidx.compose.material.icons.filled.Logout
import androidx.compose.material.icons.filled.Movie
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Storage
import androidx.compose.material.icons.filled.Sync
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import io.narl.protonstream.native.NativeRuntime
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.TonalButton
import uniffi.pstr_android.AccountState
import uniffi.pstr_android.DriveEntryRecord
import uniffi.pstr_android.DrivePlaceRecord
import uniffi.pstr_android.PlaceType
import uniffi.pstr_android.verificationToken

/** What the account card and its dialogs can ask the model for. */
internal data class AccountActions(
    val signIn: (String, String, String?) -> Unit,
    val submitSecondFactor: (String) -> Unit,
    val submitMailboxPassword: (String) -> Unit,
    val cancelSignIn: () -> Unit,
    val dismissVerification: () -> Unit,
    val signOut: () -> Unit,
    val syncNow: () -> Unit,
    val addFolder: (String, String, String) -> Unit,
    val onError: (Throwable) -> Unit,
)

/**
 * The Proton account, at the top of the shares page: why one would sign in,
 * or who is signed in and how watch-history sync is doing.
 */
@Composable
internal fun AccountCard(
    account: AccountUiState,
    actions: AccountActions,
    onSignIn: () -> Unit,
    onBrowse: () -> Unit,
) {
    val state = account.state ?: return
    Card(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
        if (state is AccountState.SignedIn) {
            var menu by remember { mutableStateOf(false) }
            ListItem(
                headlineContent = { Text(state.username, fontWeight = FontWeight.SemiBold) },
                supportingContent = { Text(syncLine(account, System.currentTimeMillis())) },
                leadingContent = {
                    Icon(Icons.Default.AccountCircle, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
                },
                trailingContent = {
                    Box {
                        IconButton(onClick = { menu = true }) {
                            Icon(Icons.Default.MoreVert, contentDescription = "Account actions")
                        }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            DropdownMenuItem(
                                text = { Text("Sync now") },
                                leadingIcon = { Icon(Icons.Default.Sync, contentDescription = null) },
                                onClick = { menu = false; actions.syncNow() },
                            )
                            DropdownMenuItem(
                                text = { Text("Sign out") },
                                leadingIcon = { Icon(Icons.Default.Logout, contentDescription = null) },
                                onClick = { menu = false; actions.signOut() },
                            )
                        }
                    }
                },
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
            )
            TonalButton(
                onClick = onBrowse,
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, bottom = 16.dp),
            ) {
                Icon(Icons.Default.Folder, contentDescription = null)
                Text("Add from your Drive", Modifier.padding(start = 8.dp))
            }
        } else {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Proton account", style = MaterialTheme.typography.titleMedium)
                Text(
                    "Optional. Signed in, watch history is kept in your own Drive so every device " +
                        "resumes where another left off, and folders of your Drive can join the library.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                TonalButton(onClick = onSignIn) { Text("Sign in") }
            }
        }
    }
}

/** "Synced 3 min ago · 2 from other devices", or why it did not. */
internal fun syncLine(account: AccountUiState, now: Long): String {
    account.syncError?.let { return "Watch history did not sync: $it" }
    val at = account.syncedAt ?: return "Watch history syncs with your Drive"
    val minutes = ((now - at).coerceAtLeast(0) / 60_000).toInt()
    val ago = when {
        minutes < 1 -> "just now"
        minutes < 60 -> "$minutes min ago"
        else -> "${minutes / 60} h ago"
    }
    val applied = if (account.applied > 0) " · ${account.applied} from other devices" else ""
    return "Synced $ago$applied"
}

/**
 * Signing in, one step at a time: address and password, then a second-factor
 * code or the mailbox password if the account has them, with Proton's CAPTCHA
 * over the top when it asks for one.
 */
@Composable
internal fun SignInDialog(account: AccountUiState, actions: AccountActions, onDismiss: () -> Unit) {
    var username by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var mailbox by remember { mutableStateOf("") }
    val state = account.state
    LaunchedEffect(state) {
        if (state is AccountState.SignedIn) onDismiss()
    }
    val dismiss = {
        if (state == AccountState.SecondFactor || state == AccountState.MailboxPassword) actions.cancelSignIn()
        onDismiss()
    }
    AlertDialog(
        onDismissRequest = dismiss,
        properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
        title = { Text("Sign in to Proton") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                when (state) {
                    AccountState.SecondFactor -> OutlinedTextField(
                        code,
                        { code = it },
                        label = { Text("Two-factor code") },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                    )
                    AccountState.MailboxPassword -> {
                        Text(
                            "This account has a separate mailbox password. It unlocks your files.",
                            style = MaterialTheme.typography.bodySmall,
                        )
                        PasswordField(mailbox, { mailbox = it }, "Mailbox password")
                    }
                    else -> {
                        OutlinedTextField(
                            username,
                            { username = it },
                            label = { Text("Address") },
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email),
                        )
                        PasswordField(password, { password = it }, "Password")
                    }
                }
                account.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                if (account.busy) CircularProgressIndicator(Modifier.padding(top = 4.dp))
            }
        },
        confirmButton = {
            when (state) {
                AccountState.SecondFactor -> AccentButton(
                    onClick = { actions.submitSecondFactor(code.trim()) },
                    enabled = code.isNotBlank() && !account.busy,
                ) { Text("Continue") }
                AccountState.MailboxPassword -> AccentButton(
                    onClick = { actions.submitMailboxPassword(mailbox) },
                    enabled = mailbox.isNotEmpty() && !account.busy,
                ) { Text("Unlock") }
                else -> AccentButton(
                    onClick = { actions.signIn(username.trim(), password, null) },
                    enabled = username.isNotBlank() && password.isNotEmpty() && !account.busy,
                ) { Text("Sign in") }
            }
        },
        dismissButton = { TonalButton(onClick = dismiss) { Text("Cancel") } },
    )
    account.verificationUrl?.let { url ->
        VerificationDialog(
            url = url,
            onToken = { token -> actions.signIn(username.trim(), password, token) },
            onDismiss = actions.dismissVerification,
        )
    }
}

@Composable
private fun PasswordField(value: String, onChange: (String) -> Unit, label: String) = OutlinedTextField(
    value,
    onChange,
    label = { Text(label) },
    singleLine = true,
    visualTransformation = PasswordVisualTransformation(),
    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
)

/**
 * Proton's hosted CAPTCHA, which it puts in front of a sign-in it does not
 * recognise — a new network, a VPN exit.
 *
 * In a WebView rather than the browser because the page reports success by
 * posting a message to its host, not by redirecting: a browser has nowhere to
 * post it back to. The page targets `window.parent`, which for a top-level
 * page is the page itself, so a listener on its own window hears it.
 */
@SuppressLint("SetJavaScriptEnabled") // The verification page is a script; it does not run without.
@Composable
private fun VerificationDialog(url: String, onToken: (String) -> Unit, onDismiss: () -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Card(Modifier.fillMaxWidth().padding(16.dp).height(560.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    "Confirm you are human",
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f).padding(start = 16.dp),
                )
                IconButton(onClick = onDismiss) { Icon(Icons.Default.Close, contentDescription = "Cancel") }
            }
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { context ->
                    val main = Handler(Looper.getMainLooper())
                    var answered = false
                    WebView(context).apply {
                        settings.javaScriptEnabled = true
                        settings.domStorageEnabled = true
                        addJavascriptInterface(
                            object {
                                @JavascriptInterface
                                fun post(message: String) {
                                    // Every message the page posts lands here,
                                    // most of them its own chatter.
                                    val token = verificationToken(message) ?: return
                                    main.post {
                                        if (!answered) {
                                            answered = true
                                            onToken(token)
                                        }
                                    }
                                }
                            },
                            "pstrVerification",
                        )
                        webViewClient = object : WebViewClient() {
                            override fun onPageStarted(view: WebView, url: String?, favicon: android.graphics.Bitmap?) {
                                view.evaluateJavascript(VERIFICATION_BRIDGE, null)
                            }

                            override fun onPageFinished(view: WebView, url: String?) {
                                view.evaluateJavascript(VERIFICATION_BRIDGE, null)
                            }

                            // The bridge above is reachable by whatever the
                            // frame shows, so the frame stays on Proton.
                            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) =
                                !isProtonPage(request.url.scheme, request.url.host)
                        }
                        loadUrl(url)
                    }
                },
            )
        }
    }
}

/** Installed once per page, at start and again at finish in case the first ran too early. */
private const val VERIFICATION_BRIDGE = """
if (!window.__pstrVerification) {
    window.__pstrVerification = true;
    window.addEventListener('message', function (event) {
        try {
            pstrVerification.post(typeof event.data === 'string' ? event.data : JSON.stringify(event.data));
        } catch (e) {}
    });
}
"""

/** Whether a page is Proton's own, over HTTPS. */
internal fun isProtonPage(scheme: String?, host: String?): Boolean =
    scheme == "https" && host != null && (host == "proton.me" || host.endsWith(".proton.me"))

/**
 * Walk the account's Drive and add a folder of it to the library.
 *
 * Full screen, because a Drive is deep and a dialog's height is a few rows of
 * it. Listings are asked for here rather than through the model: they are
 * this dialog's alone, and gone when it closes.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun DriveBrowserDialog(
    actions: AccountActions,
    onDismiss: () -> Unit,
) {
    val trail = remember { mutableStateListOf<Pair<String, DrivePlaceRecord>>() }
    var places by remember { mutableStateOf<List<DrivePlaceRecord>?>(null) }
    var entries by remember { mutableStateOf<List<DriveEntryRecord>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val current = trail.lastOrNull()?.second

    LaunchedEffect(current) {
        error = null
        if (current == null) {
            if (places == null) {
                runCatching { NativeRuntime.engine().drivePlaces() }
                    .onSuccess { places = it }
                    .onFailure { error = it.message }
            }
        } else {
            entries = null
            runCatching { NativeRuntime.engine().driveFolder(current.volumeId, current.linkId) }
                .onSuccess { entries = it }
                .onFailure { error = it.message }
        }
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        BackHandler(enabled = trail.isNotEmpty()) { trail.removeAt(trail.lastIndex) }
        Scaffold(
            topBar = {
                TopAppBar(
                    title = {
                        Text(
                            trail.lastOrNull()?.first ?: "Your Drive",
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    },
                    navigationIcon = {
                        IconButton(onClick = { if (trail.isEmpty()) onDismiss() else trail.removeAt(trail.lastIndex) }) {
                            Icon(
                                if (trail.isEmpty()) Icons.Default.Close else Icons.AutoMirrored.Filled.ArrowBack,
                                contentDescription = if (trail.isEmpty()) "Close" else "Up a folder",
                            )
                        }
                    },
                )
            },
            bottomBar = {
                // A folder already in the library is refused by the engine,
                // with a message saying so.
                val folder = current ?: return@Scaffold
                Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.End) {
                    AccentButton(
                        onClick = {
                            actions.addFolder(trail.last().first, folder.volumeId, folder.linkId)
                            onDismiss()
                        },
                    ) { Text("Add this folder") }
                }
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding)) {
                val loading = error == null && (if (current == null) places == null else entries == null)
                when {
                    error != null -> EmptyState("Could not list this", error.orEmpty())
                    loading -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                    current == null -> LazyColumn(contentPadding = PaddingValues(vertical = 8.dp)) {
                        items(places.orEmpty(), key = { "${it.volumeId}/${it.linkId}" }) { place ->
                            DriveRow(
                                icon = when (place.kind) {
                                    PlaceType.MY_FILES -> Icons.Default.Storage
                                    PlaceType.DEVICE -> Icons.Default.Computer
                                    PlaceType.SHARED_WITH_ME -> Icons.Default.FolderShared
                                },
                                name = place.name,
                                detail = null,
                                onClick = { trail.add(place.name to place) },
                            )
                        }
                    }
                    entries.orEmpty().isEmpty() -> EmptyState("Empty folder", "Nothing to add from here.")
                    else -> LazyColumn(contentPadding = PaddingValues(vertical = 8.dp)) {
                        items(entries.orEmpty(), key = { "${it.volumeId}/${it.linkId}" }) { entry ->
                            DriveRow(
                                icon = when {
                                    entry.isFolder -> Icons.Default.Folder
                                    entry.isVideo -> Icons.Default.Movie
                                    else -> Icons.Default.InsertDriveFile
                                },
                                name = entry.name,
                                detail = entry.size?.let(::formatBytes),
                                onClick = if (entry.isFolder) {
                                    {
                                        trail.add(
                                            entry.name to DrivePlaceRecord(
                                                PlaceType.MY_FILES,
                                                entry.name,
                                                entry.volumeId,
                                                entry.linkId,
                                            ),
                                        )
                                    }
                                } else {
                                    null
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun DriveRow(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    name: String,
    detail: String?,
    onClick: (() -> Unit)?,
) {
    ListItem(
        headlineContent = { Text(name, maxLines = 2, overflow = TextOverflow.Ellipsis) },
        supportingContent = detail?.let { { Text(it) } },
        leadingContent = {
            Icon(
                icon,
                contentDescription = null,
                tint = if (onClick != null) {
                    MaterialTheme.colorScheme.primary
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
            )
        },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier,
    )
}
