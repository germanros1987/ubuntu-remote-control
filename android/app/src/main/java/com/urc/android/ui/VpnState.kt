package com.urc.android.ui

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities

/**
 * VPN-presence HINT only. Reachability via [com.urc.android.proxy.Preflight] is
 * the real ground truth for "can I connect"; this just lets us show a more
 * helpful error ("Tailscale looks off") when a preflight fails. We deliberately
 * do NOT block on this — a tailnet can be up without this reporting TRANSPORT_VPN
 * on every device/OS combo.
 */
object VpnState {

    fun hasActiveVpn(context: Context): Boolean {
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
            ?: return false
        val network = cm.activeNetwork ?: return false
        val caps = cm.getNetworkCapabilities(network) ?: return false
        return caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)
    }
}
