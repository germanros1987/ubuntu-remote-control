//! Local TCP listener → TLS → remote URC agent (self-signed cert accepted on tailnet).

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tracing::{debug, info};

#[derive(Debug)]
struct TailnetTlsVerifier;

impl ServerCertVerifier for TailnetTlsVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
        ]
    }
}

fn tls_connector() -> Result<TlsConnector> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TailnetTlsVerifier))
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

fn tls_server_name(host: &str) -> Result<ServerName<'static>> {
    ServerName::try_from(host.to_string())
        .or_else(|_| ServerName::try_from("urc-agent".to_string()))
        .context("invalid server name for TLS")
}

/// Verify the remote PC exposes VNC inside TLS before opening a viewer.
pub async fn preflight_remote_vnc(remote_host: &str, remote_port: u16) -> Result<()> {
    let connector = tls_connector()?;
    let server_name = tls_server_name(remote_host)?;

    let stream = timeout(Duration::from_secs(12), TcpStream::connect((remote_host, remote_port)))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timeout connecting to {remote_host}:{remote_port}\n\
                 • Is the PC online on Tailscale?\n\
                 • Is urc-agent running? (on the PC: sudo systemctl status urc-agent)\n\
                 • Is someone logged into the desktop? (VNC starts after login)"
            )
        })?
        .with_context(|| format!("connect to {remote_host}:{remote_port}"))?;

    let mut tls = timeout(
        Duration::from_secs(12),
        connector.connect(server_name, stream),
    )
    .await
    .map_err(|_| anyhow::anyhow!("TLS handshake timeout with {remote_host}:{remote_port}"))?
    .with_context(|| format!("TLS to {remote_host}:{remote_port}"))?;

    let mut buf = [0u8; 12];
    timeout(Duration::from_secs(8), tls.read_exact(&mut buf))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timeout waiting for VNC from {remote_host}:{remote_port}\n\
                 urc-agent may be up but VNC is not (no desktop session / x0vncserver not started)"
            )
        })??;

    if !buf.starts_with(b"RFB") {
        anyhow::bail!(
            "{remote_host}:{remote_port} is reachable but did not return a VNC banner.\n\
             Check urc-agent logs on the PC: journalctl -u urc-agent -e"
        );
    }

    info!(
        host = remote_host,
        port = remote_port,
        banner = %String::from_utf8_lossy(&buf).trim(),
        "remote VNC OK"
    );
    Ok(())
}

/// After the local forwarder is listening, verify localhost:port speaks VNC through the tunnel.
pub async fn probe_local_vnc(local_port: u16) -> Result<()> {
    let stream = timeout(
        Duration::from_secs(12),
        TcpStream::connect(("127.0.0.1", local_port)),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "local tunnel on 127.0.0.1:{local_port} did not accept a connection in time"
        )
    })??;

    let mut stream = stream;
    let mut buf = [0u8; 12];
    timeout(Duration::from_secs(8), stream.read_exact(&mut buf))
        .await
        .map_err(|_| anyhow::anyhow!("timeout reading VNC banner on 127.0.0.1:{local_port}"))??;

    if !buf.starts_with(b"RFB") {
        anyhow::bail!("127.0.0.1:{local_port} is open but is not VNC");
    }

    info!(port = local_port, "local VNC tunnel OK");
    Ok(())
}

/// Listen on 127.0.0.1:`local_port` and forward to `remote_host`:`remote_port` over TLS.
pub async fn spawn_tls_forward(
    remote_host: &str,
    remote_port: u16,
    local_port: u16,
) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", local_port))
        .await
        .with_context(|| format!("bind local port {local_port} (is another urc connect still running?)"))?;
    let remote_host = remote_host.to_string();
    let connector = tls_connector()?;

    tokio::spawn(async move {
        loop {
            let Ok((mut local, _)) = listener.accept().await else {
                continue;
            };
            let host = remote_host.clone();
            let connector = connector.clone();
            tokio::spawn(async move {
                if let Err(e) = pipe_session(&host, remote_port, &connector, &mut local).await {
                    debug!(error = %e, "TLS forward session ended");
                }
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

async fn pipe_session(
    remote_host: &str,
    remote_port: u16,
    connector: &TlsConnector,
    local: &mut TcpStream,
) -> Result<()> {
    let server_name = tls_server_name(remote_host)?;
    let remote = TcpStream::connect((remote_host, remote_port))
        .await
        .with_context(|| format!("connect to {remote_host}:{remote_port}"))?;
    let mut tls = connector
        .connect(server_name, remote)
        .await
        .with_context(|| format!("TLS to {remote_host}:{remote_port}"))?;

    let (mut lr, mut lw) = local.split();
    let (mut tr, mut tw) = tokio::io::split(tls);

    let l2t = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = lr.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            tw.write_all(&buf[..n]).await?;
        }
        Ok::<(), anyhow::Error>(())
    };

    let t2l = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = tr.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            lw.write_all(&buf[..n]).await?;
        }
        Ok::<(), anyhow::Error>(())
    };

    tokio::try_join!(l2t, t2l)?;
    Ok(())
}
