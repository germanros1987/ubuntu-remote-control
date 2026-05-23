//! Standalone urc-web for local development / smoke testing.
//!
//! Run from the repo root:
//!     cargo run -p urc-web --example standalone -- /tmp 5900 18080

use std::net::SocketAddr;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let mut args = std::env::args().skip(1);
    let files_root = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let vnc_port: u16 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5900);
    let port: u16 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(18080);
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let desktop = urc_web::DesktopSession {
        username: std::env::var("USER").unwrap_or_else(|_| "nobody".into()),
        display: std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()),
        xauthority: std::env::var("XAUTHORITY").ok(),
    };
    let handle = urc_web::spawn_web_server(files_root, addr, vnc_port, desktop).await?;
    println!("urc-web standalone on http://{addr}/");
    handle.await?;
    Ok(())
}
