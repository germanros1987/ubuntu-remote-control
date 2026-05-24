package com.urc.android.discovery

import com.urc.android.proxy.Cgnat
import org.json.JSONObject

/**
 * A saved remote PC. [host] is the Tailscale 100.x address actually dialed;
 * [magicDns] is an optional friendlier name shown in the UI; [name] is a
 * user/QR-supplied label.
 */
data class Host(
    val host: String,
    val port: Int,
    val magicDns: String? = null,
    val name: String? = null,
) {
    /** Stable identity for list dedupe: a PC is identified by ip:port. */
    val key: String get() = "$host:$port"

    /** Best human label for the list / notification. */
    val displayName: String
        get() = name?.takeIf { it.isNotBlank() }
            ?: magicDns?.takeIf { it.isNotBlank() }
            ?: host

    /** A host is connectable only if it is a tailnet address. */
    val isTailnet: Boolean get() = Cgnat.isTailnetAddress(host)

    fun toJson(): JSONObject = JSONObject().apply {
        put("host", host)
        put("port", port)
        magicDns?.let { put("magicdns", it) }
        name?.let { put("name", it) }
    }

    companion object {
        fun fromJson(o: JSONObject): Host = Host(
            host = o.getString("host"),
            port = o.optInt("port", 0),
            magicDns = o.optString("magicdns", "").ifBlank { null },
            name = o.optString("name", "").ifBlank { null },
        )
    }
}
