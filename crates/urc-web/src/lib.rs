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
    routing::get,
    Router,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use rust_embed::RustEmbed;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tower_http::trace::TraceLayer;
use tracing::{debug, info, warn};

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/static"]
struct StaticAssets;

/// Build the unified web router.
///
/// * `files_root` — directory exposed by the `/api/*` file API.
/// * `vnc_port` — localhost TCP port where the VNC server is listening; the
///   WebSocket bridge proxies raw bytes there.
pub fn web_router(files_root: PathBuf, vnc_port: u16) -> Router {
    // vnc_port is captured by the handler closure — no shared State extractor needed,
    // so this router stays state-less and can be merged with `urc_files::files_router`
    // (which carries its own state internally).
    let ws_route = Router::new().route(
        "/ws/vnc",
        get(move |ws: WebSocketUpgrade| async move { vnc_ws_handler(ws, vnc_port).await }),
    );

    Router::new()
        .route("/", get(serve_index))
        .route("/{*path}", get(serve_asset))
        .merge(ws_route)
        .nest("/api", urc_files::files_router(files_root))
        .layer(TraceLayer::new_for_http())
}

/// Bind on `addr` and run the unified web server.
pub async fn spawn_web_server(
    files_root: PathBuf,
    addr: SocketAddr,
    vnc_port: u16,
) -> Result<JoinHandle<()>> {
    let app = web_router(files_root, vnc_port);
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, vnc_port, "urc-web listening (unified VNC + files + SPA)");
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!(error = %e, "urc-web server exited");
        }
    }))
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
