package com.urc.android.proxy

import android.util.Log
import java.io.IOException
import javax.net.ssl.SSLSocket

/**
 * Reachability check, ported from `preflight_remote_web`
 * (crates/urc-client/src/tls_forward.rs:85-139): open a TLS socket straight to
 * the agent, send a minimal `GET /api/health` request, and confirm a `200`.
 * This is the GROUND TRUTH for "is the PC sharing right now" — more reliable
 * than any VPN-state hint.
 */
object Preflight {

    sealed class Result {
        object Ok : Result()
        /** Host is not a tailnet (100.64.0.0/10) address — never dialed. */
        object NotTailnet : Result()
        /** TLS/connect failed or health check didn't return 200. */
        data class Unreachable(val detail: String) : Result()
    }

    private const val CONNECT_TIMEOUT_MS = 12_000
    private const val READ_TIMEOUT_MS = 8_000

    fun check(host: String, port: Int): Result {
        // GUARD: only ever dial tailnet addresses (trust-all TLS is scoped here).
        if (!Cgnat.isTailnetAddress(host)) return Result.NotTailnet

        var socket: SSLSocket? = null
        return try {
            val factory = TrustAllTlsContext.socketFactory()
            socket = factory.createSocket() as SSLSocket
            socket.connect(java.net.InetSocketAddress(host, port), CONNECT_TIMEOUT_MS)
            socket.soTimeout = READ_TIMEOUT_MS
            socket.startHandshake()

            val req = "GET /api/health HTTP/1.1\r\nHost: urc-agent\r\nConnection: close\r\n\r\n"
            socket.outputStream.write(req.toByteArray(Charsets.US_ASCII))
            socket.outputStream.flush()

            val buf = ByteArray(64)
            val n = socket.inputStream.read(buf)
            if (n <= 0) return Result.Unreachable("no health response")
            val head = String(buf, 0, n, Charsets.US_ASCII)
            if (head.startsWith("HTTP/1.1 200") || head.startsWith("HTTP/1.0 200")) {
                Log.i(TAG, "remote urc-web OK at $host:$port")
                Result.Ok
            } else {
                Result.Unreachable("health returned: ${head.lineSequence().firstOrNull().orEmpty()}")
            }
        } catch (e: IOException) {
            Result.Unreachable(e.message ?: "connect failed")
        } catch (e: Exception) {
            Result.Unreachable(e.message ?: "preflight error")
        } finally {
            try {
                socket?.close()
            } catch (_: IOException) {
            }
        }
    }

    private const val TAG = "Preflight"
}
