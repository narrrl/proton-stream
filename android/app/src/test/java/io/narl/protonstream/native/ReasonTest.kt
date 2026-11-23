package io.narl.protonstream.native

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.pstr_android.BridgeException

class ReasonTest {
    @Test
    fun `a bridge failure shows its reason without the field name in front`() {
        assertEquals("That link is already in the library", BridgeException.Failure("That link is already in the library").reason)
    }

    @Test
    fun `any other failure shows its message`() {
        assertEquals("timed out", IllegalStateException("timed out").reason)
    }
}
