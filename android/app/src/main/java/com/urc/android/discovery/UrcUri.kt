package com.urc.android.discovery

import android.net.Uri
import com.urc.android.proxy.Cgnat
import com.urc.android.proxy.DEFAULT_WEB_TLS_PORT

/**
 * Parses the pairing URL produced by `urc share` on the desktop and embedded in
 * the QR code / deep link. Contract locked with the desktop emitter:
 *
 *   urc://connect?host=100.x.y.z&magicdns=my-pc.tailnet.ts.net&port=15901&name=My%20PC
 *
 *   - scheme = `urc`, host-part = `connect`
 *   - ALL query values are RFC-3986 percent-encoded; we URL-DECODE host /
 *     magicdns / port / name (Android's [Uri.getQueryParameter] decodes for us).
 *   - we DIAL `host` (the 100.x, CGNAT-guarded downstream); `magicdns` + `name`
 *     are kept for display only. No token/secret is present in the payload.
 *
 * `host` is required and must be a tailnet address; everything else is optional.
 * The same payload is used by [QrScanActivity] and the urc:// VIEW intent
 * filter, so both paths funnel through here.
 */
object UrcUri {

    fun parse(raw: String): Host? {
        val uri = try {
            Uri.parse(raw.trim())
        } catch (e: Exception) {
            return null
        }
        if (!uri.scheme.equals("urc", ignoreCase = true)) return null
        // The locked form is hierarchical (urc://connect?…) so the authority is
        // `connect`. Tolerate an opaque urc:connect?… variant too, where the
        // action lives in the scheme-specific part.
        val action = uri.host ?: uri.schemeSpecificPart?.substringBefore('?')
        if (action != null && !action.equals("connect", ignoreCase = true)) return null

        // getQueryParameter only works on hierarchical URIs; for an opaque URI we
        // decode the raw query ourselves so a scanned opaque variant still parses.
        val params: (String) -> String? = if (uri.isHierarchical) {
            { uri.getQueryParameter(it) }
        } else {
            val parsed = parseRawQuery(uri.schemeSpecificPart?.substringAfter('?', ""))
            parsed::get
        }

        val host = params("host")?.trim().orEmpty()
        if (host.isEmpty()) return null
        // SECURITY: `host` must be a numeric IPv4 LITERAL — never a hostname. A
        // name in this slot would be a DNS-rebinding vector (it would resolve at
        // dial time, possibly off-tailnet, despite trust-all TLS). The magicdns
        // name is for display only and lives in its own field. Reject anything
        // that isn't a dotted-quad literal here, before it can be saved or dialed.
        if (Cgnat.parseIpv4Literal(host) == null) return null

        val port = params("port")?.toIntOrNull()?.takeIf { it in 1..65535 }
            ?: DEFAULT_WEB_TLS_PORT
        val magicDns = params("magicdns")?.trim()?.ifBlank { null }
        val name = params("name")?.trim()?.ifBlank { null }

        return Host(host = host, port = port, magicDns = magicDns, name = name)
    }

    /** Decode an `a=b&c=d` query whose values are percent-encoded (RFC-3986). */
    private fun parseRawQuery(query: String?): Map<String, String> {
        if (query.isNullOrEmpty()) return emptyMap()
        return query.split('&').mapNotNull { pair ->
            val eq = pair.indexOf('=')
            if (eq <= 0) return@mapNotNull null
            val key = Uri.decode(pair.substring(0, eq))
            val value = Uri.decode(pair.substring(eq + 1))
            key to value
        }.toMap()
    }
}
