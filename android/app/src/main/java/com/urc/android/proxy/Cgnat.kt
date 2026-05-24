package com.urc.android.proxy

/**
 * Tailscale assigns every node an address in the CGNAT range 100.64.0.0/10
 * (RFC 6598). Because our TLS layer is trust-all (no cert validation — see
 * [TrustAllTlsContext]), we MUST refuse to dial anything outside the tailnet.
 * This is the second non-negotiable security boundary alongside the loopback
 * bind in [LocalTlsProxy].
 *
 * 100.64.0.0/10 == addresses whose first octet is 100 and whose second octet
 * is in [64, 127].
 *
 * SECURITY: validation is on the IPv4 LITERAL only — we deliberately never call
 * InetAddress.getByName()/DNS. Resolving a hostname here would (a) be a
 * DNS-rebinding hole: an attacker-controlled name could pass the check, then
 * resolve to a different (non-tailnet) IP at dial time (TOCTOU), and (b) do
 * blocking network I/O on the caller's thread (ANR on the main-thread start
 * path). Hosts that are not numeric IPv4 literals are rejected outright at the
 * UrcUri / manual-add boundary, so only literals ever reach here, and the exact
 * validated string is what LocalTlsProxy/Preflight dial — no re-resolution.
 */
object Cgnat {

    /**
     * True iff [host] is a numeric IPv4 literal inside 100.64.0.0/10. Pure string
     * parse — NO DNS resolution. Anything that is not a bare dotted-quad literal
     * (hostnames, IPv6, garbage) returns false.
     */
    fun isTailnetAddress(host: String): Boolean {
        val octets = parseIpv4Literal(host) ?: return false
        // 100.64.0.0/10: first octet 100, second octet in [64, 127] inclusive.
        return octets[0] == 100 && octets[1] in 64..127
    }

    /**
     * Parse a strict dotted-quad IPv4 literal into four ints in [0, 255], or null
     * if [s] is not exactly four decimal octets. No leading '+', no whitespace,
     * no leading zeros beyond a single "0", no DNS — matches what
     * android.net.InetAddresses.isNumericAddress would accept for IPv4, without
     * the API-29 floor (minSdk is 26).
     */
    fun parseIpv4Literal(s: String): IntArray? {
        val parts = s.split('.')
        if (parts.size != 4) return null
        val out = IntArray(4)
        for (i in 0 until 4) {
            val p = parts[i]
            // Reject empty, non-digit, or zero-padded segments ("01", "00").
            if (p.isEmpty() || p.length > 3) return null
            if (p.any { it !in '0'..'9' }) return null
            if (p.length > 1 && p[0] == '0') return null
            val v = p.toIntOrNull() ?: return null
            if (v !in 0..255) return null
            out[i] = v
        }
        return out
    }
}
