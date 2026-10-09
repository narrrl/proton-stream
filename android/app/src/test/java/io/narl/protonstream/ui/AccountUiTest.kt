package io.narl.protonstream.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AccountUiTest {
    @Test
    fun `the sync line says how long ago and what came in`() {
        val minute = 60_000L
        assertEquals("Library, settings and watch history sync through your Drive", syncLine(AccountUiState(), 0))
        assertEquals("Synced just now", syncLine(AccountUiState(syncedAt = 0), 30_000))
        assertEquals("Synced 5 min ago", syncLine(AccountUiState(syncedAt = 0), 5 * minute))
        assertEquals(
            "Synced 2 h ago · 3 from other devices",
            syncLine(AccountUiState(syncedAt = 0, applied = 3), 120 * minute),
        )
    }

    @Test
    fun `a failed sync is said in place of when it last worked`() {
        assertEquals(
            "Did not sync: offline",
            syncLine(AccountUiState(syncedAt = 0, syncError = "offline"), 0),
        )
    }

    @Test
    fun `the verification page may only navigate within Proton over https`() {
        assertTrue(isProtonPage("https", "verify.proton.me"))
        assertTrue(isProtonPage("https", "proton.me"))
        assertFalse(isProtonPage("http", "verify.proton.me"))
        assertFalse(isProtonPage("https", "proton.me.example.com"))
        assertFalse(isProtonPage("https", "notproton.me"))
        assertFalse(isProtonPage("https", null))
    }
}
