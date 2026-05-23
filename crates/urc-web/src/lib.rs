//! Unified web UI for Ubuntu Remote Control.
//!
//! Serves a single-page app (vendored noVNC + file browser panel) plus a
//! WebSocket-to-VNC bridge that pipes binary frames to the local x0tigervncserver
//! on TCP 5900. Files API is mounted at `/api/*` from `urc-files`.
//!
//! Static assets (index.html, app.js, style.css, vendored noVNC) are embedded
//! into the binary via `rust-embed` so the agent ships as a single executable
//! and curl-installs don't have to copy a static directory.

use anyhow::Result;
use axum::{
    body::Body,
    extract::{ws::Message, Path, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use rust_embed::RustEmbed;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tower_http::trace::TraceLayer;
use tracing::{debug, info, warn};

/// Identity for running clipboard helpers (xclip) inside the user's X session.
#[derive(Clone)]
pub struct DesktopSession {
    pub username: String,
    pub display: String,
    pub xauthority: Option<String>,
}

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/static"]
struct StaticAssets;

/// Build the unified web router.
///
/// * `files_root` — directory exposed by the `/api/*` file API.
/// * `vnc_port` — localhost TCP port where the VNC server is listening; the
///   WebSocket bridge proxies raw bytes there.
/// * `desktop` — the desktop user + display so the clipboard endpoint can run
///   `xclip` inside that X session.
pub fn web_router(files_root: PathBuf, vnc_port: u16, desktop: DesktopSession) -> Router {
    // vnc_port is captured by the handler closure — no shared State extractor needed,
    // so this router stays state-less and can be merged with `urc_files::files_router`
    // (which carries its own state internally).
    let ws_route = Router::new().route(
        "/ws/vnc",
        get(move |ws: WebSocketUpgrade| async move { vnc_ws_handler(ws, vnc_port).await }),
    );

    let clipboard_user_post = desktop.clone();
    let clipboard_user_get = desktop.clone();
    let clipboard_route = Router::new()
        .route(
            "/api/clipboard",
            post(move |body: String| {
                let d = clipboard_user_post.clone();
                async move { set_clipboard_handler(d, body).await }
            })
            .get(move || {
                let d = clipboard_user_get.clone();
                async move { get_clipboard_handler(d).await }
            }),
        );

    Router::new()
        .route("/", get(serve_index))
        .merge(ws_route)
        .merge(clipboard_route)
        .nest("/api", urc_files::files_router(files_root))
        .route("/{*path}", get(serve_asset))
        .layer(TraceLayer::new_for_http())
}

/// Bind on `addr` and run the unified web server.
pub async fn spawn_web_server(
    files_root: PathBuf,
    addr: SocketAddr,
    vnc_port: u16,
    desktop: DesktopSession,
) -> Result<JoinHandle<()>> {
    let app = web_router(files_root, vnc_port, desktop);
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, vnc_port, "urc-web listening (unified VNC + files + SPA)");
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!(error = %e, "urc-web server exited");
        }
    }))
}

/// Read the desktop user's X CLIPBOARD selection. Used by the browser to poll
/// for remote-side copies (the legacy RFB clipboard channel doesn't fire on
/// Ubuntu 24.04's x0vncserver, so we read the selection directly).
async fn get_clipboard_handler(desktop: DesktopSession) -> Response {
    match read_xclip(&desktop, "clipboard").await {
        Ok(text) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            text,
        )
            .into_response(),
        Err(e) => {
            // Empty selection trips a non-zero exit; treat that as "no clipboard yet"
            // so the browser stays silent instead of flashing an error.
            if e.to_string().contains("exit code") || e.to_string().contains("no such") {
                return (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    String::new(),
                )
                    .into_response();
            }
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn read_xclip(desktop: &DesktopSession, selection: &str) -> Result<String> {
    use tokio::process::Command;
    let running_as_target =
        std::env::var("USER").ok().as_deref() == Some(desktop.username.as_str());

    let mut cmd = if running_as_target {
        let mut c = Command::new("xclip");
        c.env("DISPLAY", &desktop.display);
        if let Some(xauth) = &desktop.xauthority {
            c.env("XAUTHORITY", xauth);
        }
        c.args(["-selection", selection, "-o"]);
        c
    } else {
        let mut c = Command::new("runuser");
        c.args(["-u", &desktop.username, "--", "env"]);
        c.arg(format!("DISPLAY={}", desktop.display));
        if let Some(xauth) = &desktop.xauthority {
            c.arg(format!("XAUTHORITY={xauth}"));
        }
        c.args(["xclip", "-selection", selection, "-o"]);
        c
    };

    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let out = cmd.output().await?;
    if !out.status.success() {
        // xclip exits non-zero when selection is empty — surface as empty string.
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Write the request body into the desktop user's X CLIPBOARD selection.
/// Fall back to PRIMARY too so Linux apps that paste from middle-click work.
async fn set_clipboard_handler(desktop: DesktopSession, text: String) -> Response {
    // CLIPBOARD is what Ctrl+V uses; PRIMARY is for middle-click paste.
    for selection in ["clipboard", "primary"] {
        if let Err(e) = run_xclip(&desktop, selection, &text).await {
            warn!(error = %e, selection, "xclip failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    (StatusCode::OK, "ok").into_response()
}

async fn run_xclip(desktop: &DesktopSession, selection: &str, text: &str) -> Result<()> {
    use tokio::process::Command;

    // Production (systemd) runs urc-agent as root → wrap with `runuser -u USER`
    // so xclip joins the desktop user's session. Dev/standalone runs as that
    // user already → spawning xclip directly is enough (runuser refuses unless
    // we're root).
    let running_as_target =
        std::env::var("USER").ok().as_deref() == Some(desktop.username.as_str());

    let mut cmd = if running_as_target {
        let mut c = Command::new("xclip");
        c.env("DISPLAY", &desktop.display);
        if let Some(xauth) = &desktop.xauthority {
            c.env("XAUTHORITY", xauth);
        }
        c.args(["-selection", selection, "-i"]);
        c
    } else {
        let mut c = Command::new("runuser");
        c.args(["-u", &desktop.username, "--", "env"]);
        c.arg(format!("DISPLAY={}", desktop.display));
        if let Some(xauth) = &desktop.xauthority {
            c.arg(format!("XAUTHORITY={xauth}"));
        }
        c.args(["xclip", "-selection", selection, "-i"]);
        c
    };

    // `xclip -i` forks and the child stays running to serve paste requests.
    // wait_with_output would hang because the daemonized child inherits the
    // stderr pipe and never closes it. Set both stdout/stderr to null and use
    // `child.wait()` so we sync only on the parent's exit.
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());

    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
        stdin.flush().await?;
        drop(stdin);
    }
    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("xclip {selection} exited {status}");
    }
    Ok(())
}

async fn serve_index() -> Response {
    serve_embedded("index.html")
}

async fn serve_asset(Path(path): Path<String>) -> Response {
    serve_embedded(&path)
}

fn serve_embedded(path: &str) -> Response {
    let Some(file) = StaticAssets::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(file.data.into_owned()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn vnc_ws_handler(ws: WebSocketUpgrade, vnc_port: u16) -> impl IntoResponse {
    // noVNC requests the "binary" subprotocol; honor it so the JS side knows
    // frames are raw RFB bytes (not base64).
    ws.protocols(["binary"])
        .on_upgrade(move |socket| async move {
            if let Err(e) = bridge_vnc(socket, vnc_port).await {
                debug!(error = %e, "vnc-ws bridge ended");
            }
        })
}

async fn bridge_vnc(socket: axum::extract::ws::WebSocket, vnc_port: u16) -> Result<()> {
    let tcp = TcpStream::connect(("127.0.0.1", vnc_port)).await?;
    let (mut tcp_r, mut tcp_w) = tcp.into_split();
    let (mut ws_tx, mut ws_rx) = socket.split();

    let to_vnc = async {
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Binary(b)) => {
                    if tcp_w.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {} // ignore Text/Ping/Pong (axum handles ping/pong itself)
                Err(_) => break,
            }
        }
        let _ = tcp_w.shutdown().await;
    };

    let from_vnc = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match tcp_r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ws_tx
                        .send(Message::Binary(buf[..n].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = ws_tx.close().await;
    };

    tokio::join!(to_vnc, from_vnc);
    Ok(())
}
