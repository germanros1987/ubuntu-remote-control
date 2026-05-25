# URC Android client

Native Android WebView client for Ubuntu Remote Control. Mirrors the desktop /
Mac client (`crates/urc-client/src/tls_forward.rs`): an **on-device localhost
TLS-forward proxy** plus a **full-screen WebView** that loads the agent's unified
UI over loopback.

This is a standalone Gradle project, intentionally **outside** the Cargo
workspace. It builds an APK for sideloading (or via the release workflow).

## Architecture

```
 WebView ──http──▶ 127.0.0.1:<ephemeral>  ──TLS──▶  100.x.y.z:15901 (urc-agent)
   (UI)            LocalTlsProxy (loopback only)     tailnet peer, self-signed cert
```

- **`proxy/LocalTlsProxy.kt`** — `ServerSocket(0)` bound to
  `InetAddress.getLoopbackAddress()`, one trust-all `SSLSocket` per accepted
  connection, two raw byte-pump threads. Content-agnostic, so plain HTTP and the
  `/ws/vnc` WebSocket upgrade both ride through as opaque bytes (port of the Rust
  splice loop).
- **`proxy/Preflight.kt`** — `GET /api/health` over TLS, expects `200`. This is
  the ground truth for "is the PC sharing right now".
- **`proxy/ProxyService.kt`** — foreground `dataSync` service that owns the
  ServerSocket + accept loop, so the tunnel survives screen-off / app background.
- **`MainActivity.kt`** — the WebView host (loads `http://127.0.0.1:<port>/`).
- **`discovery/*`** — saved-host DataStore, `urc://` parsing, CameraX + ML Kit QR
  scanner.
- **`ui/*`** — host list, manual-add, connect decision, Tailscale-needed screen.

## Security model (two non-negotiable boundaries)

1. **Loopback bind.** `LocalTlsProxy` binds `InetAddress.getLoopbackAddress()`,
   never `0.0.0.0`. No other device on the LAN/Wi-Fi can reach the proxy.
2. **CGNAT guard.** TLS is **trust-all** (the agent's cert is self-signed). That
   is only safe because every dial is gated by `Cgnat.isTailnetAddress()` —
   addresses must be inside `100.64.0.0/10` (Tailscale's range). Confidentiality
   and authentication come from Tailscale's WireGuard layer underneath. The guard
   is enforced in `LocalTlsProxy.start()`, re-checked per connection, and in
   `Preflight.check()`. The check is on the **IPv4 literal only** — it never does
   DNS resolution. Hostnames (including MagicDNS names) are rejected at the
   `UrcUri` parse and manual-add boundaries, so a name can never reach a dial.
   This closes a DNS-rebinding/TOCTOU hole (a name could otherwise pass the check
   then resolve off-tailnet at dial time) and avoids blocking DNS on the UI
   thread. MagicDNS names are kept for display only.

Other hardening:

- The WebView loads **http** on `127.0.0.1` only — never https. Chromium treats
  `127.0.0.1` as a potentially-trustworthy (secure) origin, so the clipboard API
  the SPA uses keeps working; loading a self-signed https URL would break the
  secure context.
- `network_security_config.xml` permits cleartext **only** for `127.0.0.1` /
  `localhost`; all real network destinations are cleartext-forbidden.
- `WebView.setWebContentsDebuggingEnabled(true)` is called **only** when
  `BuildConfig.DEBUG` — never in release builds.

### JavaScript bridge (`window.UrcNative`)

`MainActivity` exposes a single `@JavascriptInterface` object, `UrcNative`,
purely to support voice dictation: the page can't reliably summon the Android
soft keyboard from JS (the off-screen-`<textarea>` focus trick is flaky), and
voice typing requires a voice IME the page can't reach. The bridge is the
**only** JS→native channel in the app, and it is intentionally tiny and
**fixed-action**. Exactly **two** methods are JS-callable (the only ones the SPA
uses):

- `showKeyboard()` — focus the WebView and raise the soft keyboard.
- `startDictation()` — raise the keyboard and, when no voice IME is detected,
  open the system IME picker + a dismissible Typeless onboarding dialog.

Everything else the bridge needs — voice-IME detection, the hard-coded
`market://` Typeless install intent, the IME-settings intent — lives in ordinary
**private** activity methods reached only from native code (e.g. `startDictation`
→ `maybeShowVoiceOnboarding`), and is **not** `@JavascriptInterface`-exposed.
Keeping the surface to two methods denies page JS even the one-bit "is Typeless
installed / is a voice IME enabled" disclosure that probe methods would leak.

**Why it's safe:** the only origin that can ever hold this object is our own
agent-served SPA on the loopback proxy — `WebViewClient.shouldOverrideUrlLoading`
punts any non-`127.0.0.1` navigation to the system browser, so an external page
never runs in this WebView. Even granting that, **no method takes a
page-supplied string that selects an intent, URL, file, or package**: every
intent target is a compile-time constant. Nothing exposes the tunnel, proxy
port, host list, cookies, or filesystem; methods don't throw across the bridge
and UI work hops to the main thread. There is **no `RECORD_AUDIO`** permission —
the voice IME owns the mic, out of this app's process. The contract is
documented at the `UrcNative` class in `MainActivity.kt`; adding a
`@JavascriptInterface` method, or a parameter that influences which
intent/file/URL is touched, would break these guarantees.

## Build

Requires the Android SDK (platform 35, build-tools 34.0.0) and JDK 17+.

```bash
cd android
./gradlew assembleDebug      # debug APK
./gradlew assembleRelease    # release APK (unsigned unless CI secrets present)
```

CI builds the release APK and attaches `urc-android.apk` to the GitHub Release on
tag (see `.github/workflows/release.yml`, job `android`). If the
`ANDROID_KEYSTORE_B64` / `ANDROID_KEY_ALIAS` / `ANDROID_KEYSTORE_PASSWORD` /
`ANDROID_KEY_PASSWORD` secrets are set, the APK is signed with `apksigner`;
otherwise an unsigned APK is published for local signing.

## Discovery / pairing

- **QR**: `urc share` on the PC prints a QR encoding
  `urc://connect?host=100.x&magicdns=…&port=15901&name=…`. Scan it in-app.
- **Deep link**: tapping the same `urc://connect?…` URL opens the app straight
  to connect (manifest `VIEW` intent filter, `scheme=urc host=connect`).
- **Manual add**: type the Tailscale IP + port as a fallback.

## Phase-2 / known limitations (need a physical device to validate)

These compile and follow the documented Android contracts, but cannot be
exercised in a headless CI/SDK-only environment — they need a real device on a
tailnet with a sharing PC:

- End-to-end tunnel: WebView ↔ LocalTlsProxy ↔ live agent (noVNC + files).
- QR scan with a real camera + ML Kit model download on first use.
- DownloadManager actually writing `/api/download` / `/api/download-zip` outputs.
- `onShowFileChooser` file picker round-trip into the SPA's upload form.
- HTML5 fullscreen (`requestFullscreen()` in `app.js`) → immersive bars.
- Foreground-service survival across screen-off on OEM battery-optimized ROMs.
- **Folder upload** is unsupported by Android's WebView file chooser
  (`webkitdirectory` is a no-op); single/multi-file upload works. Folder upload
  remains a phase-2 item (would need a custom JS bridge or SAF tree picker).
- **Voice dictation bridge** (`window.UrcNative`): the soft keyboard actually
  appearing on `showKeyboard()`, the IME picker opening on `startDictation()`,
  the `voiceImeReady()` heuristic, and the Typeless onboarding dialog all need a
  physical device with real IMEs to validate.
