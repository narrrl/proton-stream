package io.narl.protonstream.download

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferRateTest {
    @Test
    fun `the first sample has no rate to report`() {
        assertEquals(0L, TransferRate().sample(4L * 1024 * 1024, 1_000))
    }

    @Test
    fun `a steady transfer reports its rate`() {
        val rate = TransferRate()
        rate.sample(0, 0)
        assertEquals(1_000_000L, rate.sample(1_000_000, 1_000))
        assertEquals(1_000_000L, rate.sample(2_000_000, 2_000))
    }

    @Test
    fun `one slow block moves the rate without halving it`() {
        val rate = TransferRate()
        rate.sample(0, 0)
        rate.sample(1_000_000, 1_000)
        val after = rate.sample(2_000_000, 3_000)
        assertTrue(after in 600_000L..999_999L)
    }

    @Test
    fun `a resumed download starting below the last sample starts over`() {
        val rate = TransferRate()
        rate.sample(0, 0)
        rate.sample(1_000_000, 1_000)
        assertEquals(1_000_000L, rate.sample(0, 2_000))
    }
}
