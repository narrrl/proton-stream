package io.narl.protonstream.native

import uniffi.pstr_android.BridgeException

/**
 * What to show for a failure. uniffi's generated `BridgeException.Failure`
 * spells its message out as `reason=…`, which put the field's name in front of
 * every refusal the app showed.
 */
internal val Throwable.reason: String?
    get() = (this as? BridgeException.Failure)?.reason ?: message
