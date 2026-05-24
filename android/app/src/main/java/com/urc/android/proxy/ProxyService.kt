package com.urc.android.proxy

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.util.Log
import com.urc.android.MainActivity
import com.urc.android.R

/**
 * Foreground Service that owns the [LocalTlsProxy]. Running as a
 * FOREGROUND_SERVICE_TYPE_DATA_SYNC service keeps the loopback ServerSocket and
 * its accept loop alive across screen-off and Activity pause, which the desktop
 * client gets for free as a long-lived process.
 *
 * Lifecycle:
 *   ACTION_START (host, port) → start proxy, go foreground, broadcast localPort
 *   ACTION_STOP               → tear down proxy, drop foreground, stop self
 */
class ProxyService : Service() {

    private val binder = LocalBinder()
    private var proxy: LocalTlsProxy? = null

    @Volatile
    var localPort: Int = -1
        private set

    @Volatile
    var connectedHost: String? = null
        private set

    inner class LocalBinder : Binder() {
        fun service(): ProxyService = this@ProxyService
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopProxy()
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return START_NOT_STICKY
            }

            else -> {
                val host = intent?.getStringExtra(EXTRA_HOST)
                val port = intent?.getIntExtra(EXTRA_PORT, -1) ?: -1
                val display = intent?.getStringExtra(EXTRA_DISPLAY) ?: host ?: "PC"
                if (host == null || port <= 0) {
                    Log.w(TAG, "start with missing host/port; stopping")
                    stopSelf()
                    return START_NOT_STICKY
                }
                startProxy(host, port, display)
            }
        }
        // If killed mid-session we do NOT auto-restart: the user must re-initiate
        // (the WebView and pairing context would be gone anyway).
        return START_NOT_STICKY
    }

    private fun startProxy(host: String, port: Int, display: String) {
        stopProxy()
        connectedHost = host
        // go foreground BEFORE binding the socket so the system never kills us
        // in the gap. The DATA_SYNC type matches the manifest declaration.
        startInForeground(display)
        try {
            val p = LocalTlsProxy(host, port)
            val lp = p.start() // throws if host is non-tailnet or bind fails
            proxy = p
            localPort = lp
            // Tell whoever is bound (MainActivity) the port is ready.
            sendBroadcast(
                Intent(ACTION_PORT_READY)
                    .setPackage(packageName)
                    .putExtra(EXTRA_LOCAL_PORT, lp),
            )
            Log.i(TAG, "proxy ready on 127.0.0.1:$lp")
        } catch (e: Exception) {
            Log.e(TAG, "failed to start proxy", e)
            sendBroadcast(
                Intent(ACTION_PORT_FAILED)
                    .setPackage(packageName)
                    .putExtra(EXTRA_ERROR, e.message ?: "proxy start failed"),
            )
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
    }

    private fun stopProxy() {
        proxy?.stop()
        proxy = null
        localPort = -1
        connectedHost = null
    }

    override fun onDestroy() {
        stopProxy()
        super.onDestroy()
    }

    private fun startInForeground(display: String) {
        createChannel()
        val tapIntent = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val tapPending = PendingIntent.getActivity(
            this, 0, tapIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val stopPending = PendingIntent.getService(
            this, 1,
            Intent(this, ProxyService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val notification: Notification = Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.notif_connected, display))
            .setContentText(getString(R.string.notif_text))
            .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
            .setOngoing(true)
            .setContentIntent(tapPending)
            .addAction(
                Notification.Action.Builder(
                    null,
                    getString(R.string.disconnect),
                    stopPending,
                ).build(),
            )
            .build()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(
                NOTIF_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
        } else {
            startForeground(NOTIF_ID, notification)
        }
    }

    private fun createChannel() {
        val nm = getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL_ID) == null) {
            nm.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_ID,
                    getString(R.string.notif_channel_name),
                    NotificationManager.IMPORTANCE_LOW,
                ),
            )
        }
    }

    companion object {
        private const val TAG = "ProxyService"
        private const val CHANNEL_ID = "urc_connection"
        private const val NOTIF_ID = 42

        const val ACTION_STOP = "com.urc.android.proxy.STOP"
        const val ACTION_PORT_READY = "com.urc.android.proxy.PORT_READY"
        const val ACTION_PORT_FAILED = "com.urc.android.proxy.PORT_FAILED"

        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        const val EXTRA_DISPLAY = "display"
        const val EXTRA_LOCAL_PORT = "local_port"
        const val EXTRA_ERROR = "error"

        fun startIntent(ctx: Context, host: String, port: Int, display: String): Intent =
            Intent(ctx, ProxyService::class.java)
                .putExtra(EXTRA_HOST, host)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_DISPLAY, display)

        fun stopIntent(ctx: Context): Intent =
            Intent(ctx, ProxyService::class.java).setAction(ACTION_STOP)
    }
}
