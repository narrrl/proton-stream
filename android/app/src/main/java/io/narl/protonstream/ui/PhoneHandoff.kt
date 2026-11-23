package io.narl.protonstream.ui

import io.narl.protonstream.native.reason
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import io.narl.protonstream.ui.theme.AccentButton
import io.narl.protonstream.ui.theme.CORNER_SHEET
import io.narl.protonstream.ui.theme.CORNER_SMALL
import io.narl.protonstream.ui.theme.TonalButton
import java.net.Inet4Address
import java.net.InetAddress
import java.net.NetworkInterface
import uniffi.pstr_android.QrRecord
import uniffi.pstr_android.ShareHandoff

/**
 * Adding a share from a phone, for a TV: a QR code for a page the TV serves
 * on the home network, where the phone pastes the link (`pstr-core::handoff`).
 *
 * The TV opens the link the way the typed form does, through [onAdd], and the
 * phone's page shows the answer, so a wrong password is fixed where the
 * keyboard is.
 */
@Composable
internal fun PhoneHandoffDialog(
    start: suspend () -> Result<ShareHandoff>,
    onAdd: suspend (String, String, String?) -> String?,
    onTypeInstead: () -> Unit,
    onDismiss: () -> Unit,
) {
    // A new code is a new hand-off: the old one is closed with its effect.
    var attempt by remember { mutableIntStateOf(0) }
    val started by produceState<Result<ShareHandoff>?>(null, attempt) { value = start() }
    val handoff = started?.getOrNull()
    val qr = remember(handoff) { handoff?.let { runCatching { it.qr() }.getOrNull() } }
    var status by remember(handoff) { mutableStateOf<HandoffStatus>(HandoffStatus.Waiting) }

    DisposableEffect(handoff) { onDispose { handoff?.stop() } }
    LaunchedEffect(handoff) {
        handoff ?: return@LaunchedEffect
        while (true) {
            val offer = handoff.nextOffer()
            if (offer == null) {
                status = HandoffStatus.Expired
                return@LaunchedEffect
            }
            status = HandoffStatus.Opening
            val refusal = runCatching { onAdd(offer.name(), offer.url(), offer.customPassword()) }
                .getOrElse { it.reason ?: "That link could not be added" }
            offer.reply(refusal)
            if (refusal == null) {
                onDismiss()
                return@LaunchedEffect
            }
            status = HandoffStatus.Refused(refusal)
        }
    }

    Dialog(
        onDismissRequest = { if (status != HandoffStatus.Opening) onDismiss() },
        properties = DialogProperties(
            // The code is a key to this TV for as long as it lasts.
            securePolicy = SecureFlagPolicy.SecureOn,
            usePlatformDefaultWidth = false,
        ),
    ) {
        PhoneHandoffCard(
            qr = qr,
            url = handoff?.url(),
            failure = started?.exceptionOrNull()?.reason,
            status = status,
            onNewCode = { attempt++ },
            onTypeInstead = onTypeInstead,
            onDismiss = onDismiss,
        )
    }
}

/** What [PhoneHandoffDialog] shows, apart from the listening. */
@Composable
internal fun PhoneHandoffCard(
    qr: QrRecord?,
    url: String?,
    failure: String?,
    status: HandoffStatus,
    onNewCode: () -> Unit,
    onTypeInstead: () -> Unit,
    onDismiss: () -> Unit,
) {
    Surface(
        shape = RoundedCornerShape(CORNER_SHEET.dp),
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        modifier = Modifier.padding(24.dp).widthIn(max = 760.dp),
    ) {
        Row(
            Modifier.padding(28.dp),
            horizontalArrangement = Arrangement.spacedBy(28.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(220.dp), contentAlignment = Alignment.Center) {
                when {
                    qr != null && url != null && status != HandoffStatus.Expired ->
                        QrCode(qr, url, Modifier.size(220.dp))
                    failure == null && status != HandoffStatus.Expired -> CircularProgressIndicator()
                    else -> BrandMark(96.dp, alpha = 0.35f)
                }
            }
            HandoffText(url, failure, status, onNewCode, onTypeInstead, onDismiss)
        }
    }
}

internal sealed interface HandoffStatus {
    data object Waiting : HandoffStatus
    data object Opening : HandoffStatus
    data object Expired : HandoffStatus
    data class Refused(val reason: String) : HandoffStatus
}

@Composable
private fun HandoffText(
    url: String?,
    failure: String?,
    status: HandoffStatus,
    onNewCode: () -> Unit,
    onTypeInstead: () -> Unit,
    onDismiss: () -> Unit,
) {
    val first = remember { FocusRequester() }
    Column(Modifier.width(420.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Add a share from your phone", style = MaterialTheme.typography.headlineSmall)
        if (url == null && failure == null) {
            Muted("Starting…")
        } else if (url == null) {
            Text(failure.orEmpty(), color = MaterialTheme.colorScheme.error)
        } else {
            Step(1, "Point your phone's camera at the code.")
            Step(2, "Paste the share link into the page that opens.")
            Step(3, "Send it. The share appears here.")
            Text(
                "Or open $url in a browser on the same network.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            when (status) {
                HandoffStatus.Waiting -> Muted("Waiting for your phone…")
                HandoffStatus.Opening -> Row(
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    Text("Opening the link from your phone…")
                }
                HandoffStatus.Expired -> Muted("This code has stopped working. Get a new one to try again.")
                is HandoffStatus.Refused -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(status.reason, color = MaterialTheme.colorScheme.error)
                    Muted("Fix it on your phone and send it again.")
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.padding(top = 8.dp)) {
            val renewable = status == HandoffStatus.Expired || failure != null
            if (renewable) {
                AccentButton(onClick = onNewCode, modifier = Modifier.focusRequester(first)) { Text("New code") }
            }
            TonalButton(
                onClick = onTypeInstead,
                enabled = status != HandoffStatus.Opening,
                modifier = if (renewable) Modifier else Modifier.focusRequester(first),
            ) { Text("Type it instead") }
            TonalButton(onClick = onDismiss, enabled = status != HandoffStatus.Opening) { Text("Close") }
        }
    }
    // A D-pad needs somewhere to start.
    LaunchedEffect(status == HandoffStatus.Expired, failure) { runCatching { first.requestFocus() } }
}

@Composable
private fun Step(number: Int, text: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier.size(28.dp).background(MaterialTheme.colorScheme.primary, RoundedCornerShape(50)),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                "$number",
                color = MaterialTheme.colorScheme.onPrimary,
                style = MaterialTheme.typography.labelLarge,
                fontWeight = FontWeight.SemiBold,
            )
        }
        Text(text, style = MaterialTheme.typography.bodyLarge)
    }
}

@Composable
private fun Muted(text: String) {
    Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/**
 * Dark on white whatever the theme, with the four-module quiet zone a scanner
 * looks for, and every module on whole pixels: a fractional size leaves hairline
 * seams some scanners trip on.
 */
@Composable
internal fun QrCode(qr: QrRecord, url: String, modifier: Modifier = Modifier) {
    val width = qr.width.toInt()
    Canvas(
        modifier
            .aspectRatio(1f)
            .background(Color.White, RoundedCornerShape(CORNER_SMALL.dp))
            .semantics { contentDescription = "QR code for $url" },
    ) {
        val cells = width + 2 * QUIET_ZONE
        val cell = kotlin.math.floor(size.minDimension / cells)
        if (cell < 1f) return@Canvas
        val origin = ((size.minDimension - cell * width) / 2f).let { kotlin.math.floor(it) }
        for (y in 0 until width) {
            for (x in 0 until width) {
                if (qr.dark[y * width + x]) {
                    drawRect(Color.Black, Offset(origin + x * cell, origin + y * cell), Size(cell, cell))
                }
            }
        }
    }
}

private const val QUIET_ZONE = 4

/** This device's address on its home network, where a phone can reach it. */
internal fun lanAddress(): String? {
    val interfaces = runCatching { NetworkInterface.getNetworkInterfaces()?.toList() }.getOrNull().orEmpty()
    return pickLanAddress(
        interfaces
            .filter { runCatching { it.isUp && !it.isLoopback && !it.isPointToPoint }.getOrDefault(false) }
            .map { it.name to it.inetAddresses.toList() },
    )
}

/**
 * The private IPv4 address a phone on the same network can reach: Ethernet
 * before Wi-Fi before anything else, and never a VPN's or a cellular modem's,
 * which a phone on the sofa cannot reach.
 */
internal fun pickLanAddress(interfaces: List<Pair<String, List<InetAddress>>>): String? =
    interfaces
        .filterNot { (name, _) -> UNREACHABLE_PREFIXES.any(name::startsWith) }
        .sortedBy { (name, _) ->
            when {
                name.startsWith("eth") -> 0
                name.startsWith("wlan") -> 1
                else -> 2
            }
        }
        .flatMap { (_, addresses) -> addresses }
        .filterIsInstance<Inet4Address>()
        .firstOrNull { it.isSiteLocalAddress }
        ?.hostAddress

private val UNREACHABLE_PREFIXES = listOf("tun", "ppp", "wg", "ipsec", "rmnet", "ccmni", "dummy", "p2p")
