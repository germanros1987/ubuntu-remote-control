//! Local TCP listener → TLS → remote URC agent (self-signed cert accepted on tailnet).

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tracing::debug;

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

/// Listen on 127.0.0.1:`local_port` and forward to `remote_host`:`remote_port` over TLS.
pub async fn spawn_tls_forward(
    remote_host: &str,
    remote_port: u16,
    local_port: u16,
) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", local_port))
        .await
        .with_context(|| format!("bind local port {local_port}"))?;
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

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    Ok(())
}

async fn pipe_session(
    remote_host: &str,
    remote_port: u16,
    connector: &TlsConnector,
    local: &mut TcpStream,
) -> Result<()> {
    let server_name = ServerName::try_from(remote_host.to_string())
        .or_else(|_| ServerName::try_from("urc-agent".to_string()))
        .context("invalid server name for TLS")?;
    let remote = TcpStream::connect((remote_host, remote_port)).await?;
    let mut tls = connector.connect(server_name, remote).await?;

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
