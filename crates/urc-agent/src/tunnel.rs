//! TLS tunnel: exposes localhost VNC over encrypted port.

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use rustls::ServerConfig;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tracing::info;
use urc_common::AgentConfig;

pub struct TlsTunnel {
    listen_port: u16,
    local_port: u16,
    label: &'static str,
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl TlsTunnel {
    /// `listen_port` — external TLS port on 0.0.0.0. `local_port` — localhost TCP
    /// destination once TLS terminates. `label` — short tag for logs ("vnc"/"web").
    pub fn new(
        _config: &AgentConfig,
        listen_port: u16,
        local_port: u16,
        label: &'static str,
    ) -> Result<Self> {
        let cert_dir = PathBuf::from("/etc/urc/tls");
        fs::create_dir_all(&cert_dir).ok();

        let cert_path = cert_dir.join("agent.crt");
        let key_path = cert_dir.join("agent.key");

        if !cert_path.exists() || !key_path.exists() {
            generate_self_signed(&cert_path, &key_path)?;
            info!(path = %cert_dir.display(), "generated self-signed TLS certificate");
        }

        Ok(Self {
            listen_port,
            local_port,
            label,
            cert_path,
            key_path,
        })
    }

    pub async fn serve(self) -> Result<()> {
        let acceptor = self.build_acceptor()?;
        let listener = TcpListener::bind(("0.0.0.0", self.listen_port))
            .await
            .with_context(|| format!("bind TLS port {}", self.listen_port))?;

        info!(
            label = self.label,
            port = self.listen_port,
            local = self.local_port,
            "TLS tunnel listening"
        );

        loop {
            let (client, addr) = listener.accept().await?;
            let acceptor = acceptor.clone();
            let local_port = self.local_port;
            let label = self.label;
            tokio::spawn(async move {
                if let Err(e) = pipe_tls_client(client, acceptor, local_port).await {
                    tracing::debug!(%addr, label, error = %e, "TLS client session ended");
                }
            });
        }
    }

    fn build_acceptor(&self) -> Result<TlsAcceptor> {
        let cert_pem = fs::read_to_string(&self.cert_path).context("read cert")?;
        let key_pem = fs::read_to_string(&self.key_path).context("read key")?;

        let certs = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .context("parse certs")?;
        let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
            .context("parse key")?
            .context("no private key")?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;

        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

async fn pipe_tls_client(client: TcpStream, acceptor: TlsAcceptor, local_port: u16) -> Result<()> {
    // Key releases are tiny writes: do not hold them behind unacknowledged data.
    client.set_nodelay(true)?;
    let mut tls = acceptor.accept(client).await?;
    let mut upstream = TcpStream::connect(("127.0.0.1", local_port)).await?;
    upstream.set_nodelay(true)?;

    // Propagate EOF to VNC so it releases held keys when the viewer closes.
    tokio::io::copy_bidirectional(&mut tls, &mut upstream).await?;
    Ok(())
}

fn generate_self_signed(cert_path: &PathBuf, key_path: &PathBuf) -> Result<()> {
    let key_pair = KeyPair::generate()?;
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "urc-agent");
    let cert = params.self_signed(&key_pair)?;

    fs::write(cert_path, cert.pem())?;
    fs::write(key_path, key_pair.serialize_pem())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn forwards_key_events_and_viewer_eof_to_vnc() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.cert.der().clone()).unwrap();
        let server = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            )
            .unwrap();
        let client = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let acceptor = TlsAcceptor::from(Arc::new(server));
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client));
        let vnc = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = vnc.local_addr().unwrap().port();
        let tunnel = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tunnel.local_addr().unwrap();
        let worker = tokio::spawn(async move {
            let (socket, _) = tunnel.accept().await.unwrap();
            pipe_tls_client(socket, acceptor, port).await.unwrap();
        });

        timeout(Duration::from_secs(3), async {
            let socket = TcpStream::connect(addr).await.unwrap();
            let mut viewer = connector
                .connect("localhost".try_into().unwrap(), socket)
                .await
                .unwrap();
            let (mut desktop, _) = vnc.accept().await.unwrap();
            desktop.write_all(b"RFB 003.008\n").await.unwrap();
            let mut banner = [0; 12];
            viewer.read_exact(&mut banner).await.unwrap();
            assert_eq!(&banner, b"RFB 003.008\n");
            // Real RFB key down/up packets, delivered as separate tiny writes.
            let down = [4, 1, 0, 0, 0, 0, 0, b'a'];
            let up = [4, 0, 0, 0, 0, 0, 0, b'a'];
            viewer.write_all(&down).await.unwrap();
            viewer.flush().await.unwrap();
            let mut received = [0; 8];
            desktop.read_exact(&mut received).await.unwrap();
            assert_eq!(received, down);
            viewer.write_all(&up).await.unwrap();
            viewer.shutdown().await.unwrap();
            let mut remaining = Vec::new();
            // Previously hung: the joined copy loops never shut down VNC's
            // write half when the viewer sent EOF, leaving held keys behind.
            desktop.read_to_end(&mut remaining).await.unwrap();
            assert_eq!(remaining, up);
            desktop.shutdown().await.unwrap();
            let mut trailing = Vec::new();
            viewer.read_to_end(&mut trailing).await.unwrap();
            assert!(trailing.is_empty());
            worker.await.unwrap();
        })
        .await
        .expect("viewer disconnect must propagate to VNC promptly");
    }
}
