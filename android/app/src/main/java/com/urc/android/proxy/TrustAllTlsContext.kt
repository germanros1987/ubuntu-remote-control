package com.urc.android.proxy

import java.security.cert.X509Certificate
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocketFactory
import javax.net.ssl.TrustManager
import javax.net.ssl.X509TrustManager

/**
 * Trust-all TLS, mirroring the desktop/Mac client's `TailnetTlsVerifier`
 * (crates/urc-client/src/tls_forward.rs:18-57). The URC agent presents a
 * self-signed certificate; on the tailnet we accept it unconditionally.
 *
 * SECURITY: this trust-all posture is ONLY safe because every dial is gated by
 * [Cgnat.isTailnetAddress] — we never open one of these sockets to an address
 * outside 100.64.0.0/10. Authentication/confidentiality is provided by
 * Tailscale's WireGuard layer underneath; the TLS here just keeps the loopback
 * proxy and the agent speaking the same protocol the browser expects.
 */
object TrustAllTlsContext {

    /** A TrustManager that accepts any certificate chain (tailnet-scoped use only). */
    private val trustAllManager = object : X509TrustManager {
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) {}
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {}
        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    private val context: SSLContext by lazy {
        SSLContext.getInstance("TLS").apply {
            init(null, arrayOf<TrustManager>(trustAllManager), java.security.SecureRandom())
        }
    }

    fun socketFactory(): SSLSocketFactory = context.socketFactory
}
