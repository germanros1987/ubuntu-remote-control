package com.urc.android.ui

import android.content.Context
import com.urc.android.discovery.Host
import com.urc.android.proxy.Preflight
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Shared connect decision used by both the host-list tap and the urc:// deep
 * link. Runs [Preflight] off the main thread (reachability is ground truth)
 * and reports a verdict the caller turns into either launching [MainActivity]
 * or showing the Tailscale-needed screen.
 */
object Connector {

    sealed class Verdict {
        /** Reachable now — caller should start the proxy + WebView. */
        object Connect : Verdict()
        /** Host is not a tailnet address — bad pairing data. */
        object NotTailnet : Verdict()
        /** Tailnet address but unreachable — likely VPN off or PC not sharing. */
        data class Unreachable(val detail: String, val vpnLikelyOff: Boolean) : Verdict()
    }

    suspend fun decide(context: Context, host: Host): Verdict = withContext(Dispatchers.IO) {
        when (val r = Preflight.check(host.host, host.port)) {
            is Preflight.Result.Ok -> Verdict.Connect
            is Preflight.Result.NotTailnet -> Verdict.NotTailnet
            is Preflight.Result.Unreachable ->
                Verdict.Unreachable(r.detail, vpnLikelyOff = !VpnState.hasActiveVpn(context))
        }
    }
}
