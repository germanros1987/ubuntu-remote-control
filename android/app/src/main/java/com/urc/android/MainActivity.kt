package com.urc.android

import android.Manifest
import android.app.DownloadManager
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.IBinder
import android.util.Log
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.DownloadListener
import android.webkit.PermissionRequest
import android.webkit.URLUtil
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import android.widget.Toast
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.urc.android.proxy.ProxyService

/**
 * Full-screen WebView host. Binds [ProxyService], waits for the loopback port,
 * then loads http://127.0.0.1:<port>/ — NEVER https, so 127.0.0.1 stays a
 * Chromium "potentially trustworthy" secure context and the clipboard/secure
 * APIs the SPA uses keep working.
 */
class MainActivity : AppCompatActivity() {

    private lateinit var container: FrameLayout
    private lateinit var webView: WebView

    private var service: ProxyService? = null
    private var bound = false

    // onShowFileChooser plumbing.
    private var filePathCallback: ValueCallback<Array<Uri>>? = null

    // onShowCustomView (HTML5 fullscreen / requestFullscreen()) plumbing.
    private var customView: View? = null
    private var customViewCallback: WebChromeClient.CustomViewCallback? = null

    private val fileChooserLauncher =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val cb = filePathCallback
            filePathCallback = null
            if (cb == null) return@registerForActivityResult
            cb.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(result.resultCode, result.data))
        }

    private val notifPermLauncher =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { /* best-effort */ }

    private val portReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context?, intent: Intent?) {
            when (intent?.action) {
                ProxyService.ACTION_PORT_READY -> {
                    val port = intent.getIntExtra(ProxyService.EXTRA_LOCAL_PORT, -1)
                    if (port > 0) loadRemote(port)
                }

                ProxyService.ACTION_PORT_FAILED -> {
                    val err = intent.getStringExtra(ProxyService.EXTRA_ERROR) ?: "connection failed"
                    Toast.makeText(this@MainActivity, err, Toast.LENGTH_LONG).show()
                    disconnectToHostList()
                }
            }
        }
    }

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
            service = (binder as? ProxyService.LocalBinder)?.service()
            bound = true
            // If the proxy already came up before we bound, load immediately.
            service?.localPort?.takeIf { it > 0 }?.let { loadRemote(it) }
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            service = null
            bound = false
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        WindowCompat.setDecorFitsSystemWindows(window, false)

        container = FrameLayout(this)
        setContentView(container)

        webView = WebView(this)
        container.addView(
            webView,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            ),
        )
        configureWebView()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notifPermLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }

        // Hardware/gesture back → disconnect to the host list, do not destroy the
        // WebView mid-frame (the desktop client's Ctrl+C analogue).
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (customView != null) {
                    // Exit HTML5 fullscreen first if a video/desktop is fullscreened.
                    (webView.webChromeClient as? FullscreenChromeClient)?.onHideCustomView()
                    return
                }
                disconnectToHostList()
            }
        })

        // Start the proxy for the host passed in (from HostListActivity).
        val host = intent.getStringExtra(EXTRA_HOST)
        val port = intent.getIntExtra(EXTRA_PORT, -1)
        val display = intent.getStringExtra(EXTRA_DISPLAY) ?: host
        if (host != null && port > 0) {
            startForeground(host, port, display ?: host)
        }
        bindService(Intent(this, ProxyService::class.java), connection, Context.BIND_AUTO_CREATE)
    }

    private fun startForeground(host: String, port: Int, display: String) {
        val intent = ProxyService.startIntent(this, host, port, display)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(intent)
        } else {
            startService(intent)
        }
    }

    @Suppress("SetJavaScriptEnabled")
    private fun configureWebView() {
        // WebView remote debugging ONLY in debug builds — never in release.
        if (BuildConfig.DEBUG) {
            WebView.setWebContentsDebuggingEnabled(true)
        }

        with(webView.settings) {
            javaScriptEnabled = true
            domStorageEnabled = true
            allowFileAccess = true
            allowContentAccess = true
            mediaPlaybackRequiresUserGesture = false
            // The SPA is same-origin loopback; allow it to do its thing.
            cacheMode = android.webkit.WebSettings.LOAD_DEFAULT
            useWideViewPort = true
            loadWithOverviewMode = true
        }
        CookieManager.getInstance().setAcceptCookie(true)
        // Single-origin loopback app — there is no third party. Keep them off.
        CookieManager.getInstance().setAcceptThirdPartyCookies(webView, false)

        webView.webViewClient = object : WebViewClient() {
            // Keep navigation inside the loopback origin in the WebView; punt
            // anything else (mailto:, external https) to the system.
            override fun shouldOverrideUrlLoading(
                view: WebView,
                request: android.webkit.WebResourceRequest,
            ): Boolean {
                val url = request.url
                if (url.host == "127.0.0.1" || url.host == "localhost") return false
                return try {
                    startActivity(Intent(Intent.ACTION_VIEW, url))
                    true
                } catch (e: Exception) {
                    true
                }
            }
        }

        webView.webChromeClient = FullscreenChromeClient()

        // Downloads (/api/download, /api/download-zip) → system DownloadManager.
        webView.setDownloadListener(DownloadListener { url, userAgent, contentDisposition, mimetype, _ ->
            enqueueDownload(url, userAgent, contentDisposition, mimetype)
        })
    }

    private fun loadRemote(port: Int) {
        val url = "http://127.0.0.1:$port/"
        Log.i(TAG, "loading $url")
        // Avoid reloading if we're already there (e.g. duplicate broadcast).
        if (webView.url == url) return
        webView.loadUrl(url)
    }

    private fun enqueueDownload(
        url: String,
        userAgent: String?,
        contentDisposition: String?,
        mimetype: String?,
    ) {
        try {
            // Derive filename from Content-Disposition when the server sends one
            // (urc-web sets it for download-zip); else fall back to the URL path.
            val fileName = URLUtil.guessFileName(url, contentDisposition, mimetype)
            val request = DownloadManager.Request(Uri.parse(url)).apply {
                setMimeType(mimetype)
                userAgent?.let { addRequestHeader("User-Agent", it) }
                // Pass loopback cookies so the agent authorizes the download.
                CookieManager.getInstance().getCookie(url)?.let { addRequestHeader("Cookie", it) }
                setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
                setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS, fileName)
            }
            val dm = getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
            dm.enqueue(request)
            Toast.makeText(this, "Downloading $fileName", Toast.LENGTH_SHORT).show()
        } catch (e: Exception) {
            Log.e(TAG, "download failed", e)
            Toast.makeText(this, "Download failed: ${e.message}", Toast.LENGTH_LONG).show()
        }
    }

    private fun disconnectToHostList() {
        // Stop the proxy + foreground service, then return to the host list.
        startService(ProxyService.stopIntent(this))
        finish()
    }

    override fun onDestroy() {
        // portReceiver is unregistered in onStop() (always called before onDestroy).
        if (bound) {
            unbindService(connection)
            bound = false
        }
        webView.destroy()
        super.onDestroy()
    }

    override fun onStart() {
        super.onStart()
        val filter = IntentFilter().apply {
            addAction(ProxyService.ACTION_PORT_READY)
            addAction(ProxyService.ACTION_PORT_FAILED)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(portReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            registerReceiver(portReceiver, filter)
        }
    }

    override fun onStop() {
        super.onStop()
        try {
            unregisterReceiver(portReceiver)
        } catch (_: Exception) {
        }
    }

    /** Chrome client: file chooser, permission grants, HTML5 fullscreen. */
    private inner class FullscreenChromeClient : WebChromeClient() {

        override fun onShowFileChooser(
            view: WebView?,
            callback: ValueCallback<Array<Uri>>?,
            params: FileChooserParams?,
        ): Boolean {
            filePathCallback?.onReceiveValue(null)
            filePathCallback = callback
            // File upload only. Folder upload is unsupported by WebView's chooser
            // (webkitdirectory is a no-op here) — phase-2 item; single/multi-file
            // works via MODE_OPEN_MULTIPLE.
            val intent = params?.createIntent() ?: Intent(Intent.ACTION_GET_CONTENT).apply {
                type = "*/*"
                addCategory(Intent.CATEGORY_OPENABLE)
            }
            return try {
                fileChooserLauncher.launch(intent)
                true
            } catch (e: Exception) {
                filePathCallback = null
                false
            }
        }

        override fun onPermissionRequest(request: PermissionRequest?) {
            // Grant clipboard (and other loopback-origin) requests; the origin is
            // our own on-device proxy, fully trusted.
            request ?: return
            runOnUiThread { request.grant(request.resources) }
        }

        override fun onShowCustomView(view: View?, callback: CustomViewCallback?) {
            if (customView != null) {
                onHideCustomView()
                return
            }
            customView = view
            customViewCallback = callback
            container.addView(
                view,
                FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                ),
            )
            webView.visibility = View.GONE
            enterImmersive(true)
        }

        override fun onHideCustomView() {
            val view = customView ?: return
            container.removeView(view)
            customView = null
            webView.visibility = View.VISIBLE
            customViewCallback?.onCustomViewHidden()
            customViewCallback = null
            enterImmersive(false)
        }
    }

    /** Toggle immersive sticky fullscreen for the requestFullscreen() path in app.js. */
    private fun enterImmersive(on: Boolean) {
        val controller = WindowInsetsControllerCompat(window, container)
        if (on) {
            controller.hide(WindowInsetsCompat.Type.systemBars())
            controller.systemBarsBehavior =
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        } else {
            controller.show(WindowInsetsCompat.Type.systemBars())
        }
    }

    companion object {
        private const val TAG = "MainActivity"
        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        const val EXTRA_DISPLAY = "display"

        fun intent(ctx: Context, host: String, port: Int, display: String): Intent =
            Intent(ctx, MainActivity::class.java)
                .putExtra(EXTRA_HOST, host)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_DISPLAY, display)
    }
}
