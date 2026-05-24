package com.urc.android.proxy

import android.util.Log
import java.io.IOException
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean
import javax.net.ssl.SSLSocket

/**
 * On-device localhost → TLS → tailnet-agent forwarder. Direct analogue of
 * `spawn_tls_forward` (crates/urc-client/src/tls_forward.rs:222-296):
 *
 *   browser/WebView ──http──▶ 127.0.0.1:localPort ──TLS──▶ host:remotePort (agent)
 *
 * Each accepted loopback connection opens its own SSLSocket to the agent and
 * runs two raw byte-pump threads. The pump is content-agnostic: plain HTTP and
 * the `/ws/vnc` WebSocket upgrade both ride through as opaque bytes, exactly
 * like the Rust splice loop.
 */
class LocalTlsProxy(
    private val remoteHost: String,
    private val remotePort: Int,
) {
    private var serverSocket: ServerSocket? = null
    private var acceptThread: Thread? = null
    private val running = AtomicBoolean(false)

    /** Loopback port the WebView should target; valid after [start]. */
    @Volatile
    var localPort: Int = -1
        private set

    /**
     * Bind a loopback ServerSocket on an ephemeral port and begin accepting.
     * @throws IllegalArgumentException if [remoteHost] is not a tailnet address.
     * @throws IOException if the loopback bind fails.
     */
    @Throws(IOException::class)
    fun start(): Int {
        // SECURITY GUARD #1 (non-negotiable): trust-all TLS means we must never
        // dial anything outside the tailnet. Refuse before we even bind.
        require(Cgnat.isTailnetAddress(remoteHost)) {
            "refusing to proxy to non-tailnet host: $remoteHost"
        }

        // SECURITY GUARD #2 (non-negotiable): bind the listener to the LOOPBACK
        // address ONLY — never 0.0.0.0. Port 0 = OS-assigned ephemeral port.
        // Binding to InetAddress.getLoopbackAddress() ensures no other device on
        // the LAN/Wi-Fi can reach this proxy; only apps on THIS device can.
        val ss = ServerSocket(0, 50, InetAddress.getLoopbackAddress())
        serverSocket = ss
        localPort = ss.localPort
        running.set(true)

        acceptThread = Thread({ acceptLoop(ss) }, "urc-proxy-accept").apply {
            isDaemon = true
            start()
        }
        Log.i(TAG, "loopback proxy listening on 127.0.0.1:$localPort → $remoteHost:$remotePort")
        return localPort
    }

    private fun acceptLoop(ss: ServerSocket) {
        while (running.get()) {
            val local: Socket = try {
                ss.accept()
            } catch (e: IOException) {
                if (running.get()) Log.w(TAG, "accept failed", e)
                break
            }
            Thread({ handleConnection(local) }, "urc-proxy-conn").apply {
                isDaemon = true
                start()
            }
        }
    }

    private fun handleConnection(local: Socket) {
        var tls: SSLSocket? = null
        try {
            // Open the upstream TLS socket to the agent. Guard re-checked here in
            // case remoteHost were ever mutated; defense in depth.
            require(Cgnat.isTailnetAddress(remoteHost)) {
                "refusing to dial non-tailnet host: $remoteHost"
            }
            val factory = TrustAllTlsContext.socketFactory()
            val s = factory.createSocket(remoteHost, remotePort) as SSLSocket
            s.startHandshake()
            // Self-signed cert: hostname check is meaningless, accept any.
            // (Verification already disabled by the trust-all SSLContext.)
            tls = s

            val l2t = pump(local, s, "l2t")
            val t2l = pump(s, local, "t2l")
            l2t.start()
            t2l.start()
            // When either direction ends, the joins below complete and we close.
            l2t.join()
            t2l.join()
        } catch (e: Exception) {
            Log.d(TAG, "session ended: ${e.message}")
        } finally {
            closeQuietly(tls)
            closeQuietly(local)
        }
    }

    /**
     * One direction of the splice. Mirrors the 8 KiB read/write loop in
     * `pipe_session` (tls_forward.rs:270-292). On EOF/error it closes BOTH
     * sockets so the opposite pump's blocked read/write throws and that thread
     * exits too — symmetric teardown, no half-open leak.
     */
    private fun pump(srcSock: Socket, dstSock: Socket, tag: String): Thread =
        Thread({
            val buf = ByteArray(8192)
            try {
                val src = srcSock.getInputStream()
                val dst = dstSock.getOutputStream()
                while (true) {
                    val n = src.read(buf)
                    if (n < 0) break
                    dst.write(buf, 0, n)
                    dst.flush()
                }
            } catch (_: IOException) {
                // Peer closed or socket torn down — normal at end of session.
            } finally {
                // Close both sockets so the other direction unblocks immediately.
                closeQuietly(srcSock)
                closeQuietly(dstSock)
            }
        }, "urc-proxy-$tag").apply { isDaemon = true }

    /** Stop accepting and close the listener. In-flight sessions drain on their own. */
    fun stop() {
        running.set(false)
        closeQuietly(serverSocket)
        serverSocket = null
        localPort = -1
    }

    private fun closeQuietly(c: java.io.Closeable?) {
        try {
            c?.close()
        } catch (_: IOException) {
        }
    }

    companion object {
        private const val TAG = "LocalTlsProxy"
    }
}
