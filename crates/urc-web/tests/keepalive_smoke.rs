use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as TMessage;
use urc_web::{spawn_web_server, DesktopSession};

#[tokio::test]
async fn idle_vnc_stream_still_pings_client() {
    // Fake VNC server: accept and then say nothing (fully idle desktop).
    let vnc_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vnc_port = vnc_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (_sock, _addr) = vnc_listener.accept().await.unwrap();
        // hold the connection open, send nothing
        tokio::time::sleep(Duration::from_secs(60)).await;
    });

    let files_root = std::env::temp_dir();
    let desktop = DesktopSession {
        username: "nobody".into(),
        display: ":0".into(),
        xauthority: None,
    };
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    // spawn_web_server binds internally; grab the actual port via a pre-bind probe.
    let probe = TcpListener::bind(addr).await.unwrap();
    let bound = probe.local_addr().unwrap();
    drop(probe);

    spawn_web_server(files_root, bound, vnc_port, desktop)
        .await
        .unwrap();

    // give the server a moment to actually bind
    tokio::time::sleep(Duration::from_millis(200)).await;

    let url = format!("ws://{}/ws/vnc", bound);
    let (ws_stream, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let (_write, mut read) = ws_stream.split();

    let got_ping = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(TMessage::Ping(_)) => return true,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    assert!(
        got_ping,
        "expected a WS Ping within 30s on an idle VNC stream"
    );
}
