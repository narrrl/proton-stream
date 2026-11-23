package io.narl.protonstream.ui

import java.net.InetAddress
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PhoneHandoffTest {
    private fun address(text: String): InetAddress = InetAddress.getByName(text)

    @Test
    fun `ethernet is preferred over wi-fi and both over anything else`() {
        val picked = pickLanAddress(
            listOf(
                "bridge0" to listOf(address("172.17.0.1")),
                "wlan0" to listOf(address("192.168.1.30")),
                "eth0" to listOf(address("192.168.1.20")),
            ),
        )
        assertEquals("192.168.1.20", picked)
    }

    @Test
    fun `a vpn or a cellular modem is never the address shown to a phone`() {
        val picked = pickLanAddress(
            listOf(
                "tun0" to listOf(address("10.8.0.2")),
                "wg0" to listOf(address("10.6.0.4")),
                "rmnet_data0" to listOf(address("10.200.1.1")),
                "wlan0" to listOf(address("192.168.178.44")),
            ),
        )
        assertEquals("192.168.178.44", picked)
    }

    @Test
    fun `a public or ipv6 address alone is no home network`() {
        val picked = pickLanAddress(
            listOf(
                "eth0" to listOf(address("fe80::1"), address("203.0.113.9")),
            ),
        )
        assertNull(picked)
    }

    @Test
    fun `the private ipv4 address is found beside an ipv6 one`() {
        val picked = pickLanAddress(listOf("wlan0" to listOf(address("fe80::a"), address("10.0.0.12"))))
        assertEquals("10.0.0.12", picked)
    }
}
