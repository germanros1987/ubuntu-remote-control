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
import android.provider.Settings
import android.util.Log
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.InputMethodManager
import android.webkit.CookieManager
import android.webkit.DownloadListener
import android.webkit.JavascriptInterface
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
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
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

    // Last system-bar insets in CSS px, cached so a freshly-loaded document (which
    // loses the inline custom properties) can be re-primed in onPageFinished.
    private var insetTopCss = 0
    private var insetRightCss = 0
    private var insetBottomCss = 0
    private var insetLeftCss = 0

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

        // Feed the REAL system-bar insets to the web layer. CSS env(safe-area-inset-*)
        // on Android only reliably covers display cutouts, NOT the system navigation
        // bar, so in landscape the 3-button nav bar overlaps the topbar/FAB. We read
        // the systemBars() insets here and publish them as --urc-inset-* custom props;
        // the SPA's CSS does max(env(...), var(--urc-inset-...,0px)).
        ViewCompat.setOnApplyWindowInsetsListener(webView) { _, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val density = resources.displayMetrics.density
            insetTopCss = (bars.top / density).toInt()
            insetRightCss = (bars.right / density).toInt()
            insetBottomCss = (bars.bottom / density).toInt()
            insetLeftCss = (bars.left / density).toInt()
            injectInsets()
            // Don't consume — let the system continue laying out the window.
            insets
        }

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
        // The page taps ⌨/🎤 then calls UrcNative.showKeyboard(); the WebView must
        // be able to take input focus for the IME to attach to it.
        webView.isFocusable = true
        webView.isFocusableInTouchMode = true

        // Native bridge for the soft-keyboard / voice-dictation onboarding. The
        // surface is intentionally tiny and side-effect-bounded — see [UrcNative].
        webView.addJavascriptInterface(UrcNative(), "UrcNative")

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

            // A fresh document loses the inline --urc-inset-* custom properties, so
            // re-prime them once the page is up (also covers orientation-change
            // reloads). Live insets changes are handled by the insets listener.
            override fun onPageFinished(view: WebView?, url: String?) {
                super.onPageFinished(view, url)
                injectInsets()
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

    /**
     * Publish the cached system-bar insets (CSS px) onto document.documentElement as
     * the four --urc-inset-* custom properties the SPA's CSS reads. Re-run on every
     * insets change AND on each onPageFinished (a fresh document drops inline vars).
     * CONTRACT: sets exactly --urc-inset-top/right/bottom/left, units 'px'.
     */
    private fun injectInsets() {
        val js = "document.documentElement.style.setProperty('--urc-inset-top','${insetTopCss}px');" +
            "document.documentElement.style.setProperty('--urc-inset-right','${insetRightCss}px');" +
            "document.documentElement.style.setProperty('--urc-inset-bottom','${insetBottomCss}px');" +
            "document.documentElement.style.setProperty('--urc-inset-left','${insetLeftCss}px');"
        webView.evaluateJavascript(js, null)
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

    /**
     * `window.UrcNative` — the ONLY JavaScript→native bridge in this app.
     *
     * JS-CALLABLE SURFACE: exactly two methods, [showKeyboard] and
     * [startDictation] (the only ones the SPA calls). Everything else the bridge
     * needs — voice-IME detection, the Typeless install intent, the IME-settings
     * intent — lives in ordinary private methods on the activity and is reached
     * only from native code (e.g. [startDictation] → [maybeShowVoiceOnboarding]),
     * never exposed to the page. Keeping the surface this small is deliberate: it
     * denies page JS even the one-bit "is Typeless installed / is a voice IME
     * enabled" disclosure those probes would otherwise leak.
     *
     * TRUST ASSUMPTION: the page calling this is our own agent-served SPA loaded
     * over the on-device loopback proxy (`http://127.0.0.1:<port>/`). That origin
     * is fully trusted (see the WebView/loopback notes in [configureWebView] and
     * README "Security model"). The WebView never navigates off-loopback —
     * [WebViewClient.shouldOverrideUrlLoading] punts any non-127.0.0.1 host to the
     * system browser, so an external page cannot end up holding this object.
     *
     * Even so the surface is deliberately TINY and FIXED-ACTION: each method only
     * toggles the soft keyboard / IME picker (and, for dictation, may open a
     * hard-coded market:// or Settings intent from native code). NOTHING here
     * takes a page-supplied string that selects an intent, URL, file, or package,
     * and nothing exposes the tunnel, proxy port, host list, cookies, or
     * filesystem. Adding a @JavascriptInterface method, or a parameter that
     * influences which intent/URL/file is touched, would break that guarantee — do
     * not. UI work hops to the main thread; methods never throw across the bridge.
     */
    private inner class UrcNative {

        /**
         * Summon the soft keyboard against the WebView. FIXES the bug where the
         * page's off-screen-<textarea>-focus trick failed to raise the IME — we
         * ask the platform [InputMethodManager] directly, which is reliable.
         */
        @JavascriptInterface
        fun showKeyboard() {
            runOnUiThread { showSoftKeyboard() }
        }

        /**
         * Raise the keyboard for dictation. Typeless being merely INSTALLED is not
         * enough — it has to be the ACTIVE input method or tapping the mic just
         * brings up the user's current (non-voice) keyboard. So: raise the keyboard,
         * then if the CURRENT default IME isn't Typeless, open the IME picker so the
         * user can switch to it. When Typeless isn't installed at all, fall back to
         * the contextual install/enable onboarding.
         */
        @JavascriptInterface
        fun startDictation() {
            runOnUiThread {
                showSoftKeyboard()
                if (isTypelessActive()) return@runOnUiThread
                if (isTypelessInstalled()) {
                    // Installed but not the active IME → let the user switch to it.
                    val imm = getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
                    imm?.showInputMethodPicker()
                } else {
                    // Not installed → contextual install/enable onboarding.
                    maybeShowVoiceOnboarding()
                }
            }
        }
    }

    /** Focus the WebView and ask the IME to appear over it. Must run on the UI thread. */
    private fun showSoftKeyboard() {
        webView.requestFocus()
        val imm = getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
        imm?.showSoftInput(webView, InputMethodManager.SHOW_IMPLICIT)
    }

    /**
     * Is Typeless the CURRENTLY ACTIVE input method? Reads the default-IME id from
     * Settings.Secure.DEFAULT_INPUT_METHOD (an id like
     * "com.typeless.mobile/.XxxService") and checks the package prefix. This is the
     * distinction that matters for dictation: installed ≠ active, and only the
     * active IME determines what comes up when the keyboard is shown. Never throws —
     * see the [UrcNative] class doc for why this isn't exposed to page JS.
     */
    private fun isTypelessActive(): Boolean =
        try {
            val current = Settings.Secure.getString(
                contentResolver,
                Settings.Secure.DEFAULT_INPUT_METHOD,
            )
            current?.startsWith(TYPELESS_PKG) == true
        } catch (e: Exception) {
            false
        }

    /** Reliable install probe (needs the <queries> entry, mirrors HostListActivity). */
    private fun isTypelessInstalled(): Boolean =
        try {
            packageManager.getPackageInfo(TYPELESS_PKG, 0)
            true
        } catch (e: PackageManager.NameNotFoundException) {
            false
        } catch (e: Exception) {
            false
        }

    /** Deep-link Typeless in the Play Store; web fallback (mirrors openTailscale). */
    private fun openTypelessInstallInternal() {
        try {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=$TYPELESS_PKG")))
        } catch (e: Exception) {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://play.google.com/store/apps/details?id=$TYPELESS_PKG")))
        }
    }

    /**
     * Contextual, dismissible onboarding shown from [UrcNative.startDictation] when
     * no voice IME looks available. Voice is optional — this is never a
     * launch-blocking card. Mirrors the Tailscale-needed dialog idioms.
     */
    private fun maybeShowVoiceOnboarding() {
        val installed = isTypelessInstalled()
        val bodyRes = if (installed) R.string.voice_enable_body else R.string.voice_install_body
        val builder = AlertDialog.Builder(this)
            .setTitle(R.string.voice_onboard_title)
            .setMessage(bodyRes)
            .setNegativeButton(R.string.cancel, null)
        if (installed) {
            // Typeless present but not the active voice keyboard → send to IME settings.
            builder.setPositiveButton(R.string.voice_enable_action) { _, _ ->
                try {
                    startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
                } catch (e: Exception) {
                    Log.e(TAG, "ime settings failed", e)
                }
            }
        } else {
            builder.setPositiveButton(R.string.voice_install_action) { _, _ -> openTypelessInstallInternal() }
                .setNeutralButton(R.string.voice_enable_action) {
                    _, _ ->
                    try {
                        startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
                    } catch (e: Exception) {
                        Log.e(TAG, "ime settings failed", e)
                    }
                }
        }
        builder.show()
    }

    companion object {
        private const val TAG = "MainActivity"
        /** Typeless voice keyboard package — kept in sync with the manifest <queries>. */
        private const val TYPELESS_PKG = "com.typeless.mobile"
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
