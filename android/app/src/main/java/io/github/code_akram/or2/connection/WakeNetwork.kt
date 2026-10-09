package io.github.code_akram.or2.connection

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities

/**
 * The subnet-directed broadcast addresses of the current network, for the Wake-on-LAN packet: each IPv4 link address
 * of its `LinkProperties` with its prefix length ([directedBroadcast]). With a VPN app up (ZeroTier) the current network
 * is the VPN, and the network it runs over is not reported to apps, so the Wi-Fi and Ethernet networks the phone is on
 * count too: the host to wake sits on the LAN underneath. Mobile data is never a LAN with a host on it, so it gives none
 * (the packet still goes to the limited broadcast).
 */
fun currentBroadcasts(context: Context): List<String> {
    val manager = context.getSystemService(ConnectivityManager::class.java) ?: return emptyList()
    val active = manager.activeNetwork ?: return emptyList()
    val local = if (manager.hasTransport(active, NetworkCapabilities.TRANSPORT_VPN)) {
        @Suppress("DEPRECATION") // The one way to list the networks under a VPN; no callback is registered for this.
        manager.allNetworks.filter {
            manager.hasTransport(it, NetworkCapabilities.TRANSPORT_WIFI) || manager.hasTransport(it, NetworkCapabilities.TRANSPORT_ETHERNET)
        }
    } else {
        emptyList()
    }
    return directedBroadcasts(
        (listOf(active) + local).distinct().filterNot { manager.hasTransport(it, NetworkCapabilities.TRANSPORT_CELLULAR) }
            .flatMap { network -> manager.getLinkProperties(network)?.linkAddresses.orEmpty().map { it.address to it.prefixLength } },
    )
}

private fun ConnectivityManager.hasTransport(network: Network, transport: Int): Boolean =
    getNetworkCapabilities(network)?.hasTransport(transport) == true
