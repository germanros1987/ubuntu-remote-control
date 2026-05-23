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
    vnc_port: u16,
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl TlsTunnel {
    pub fn new(config: &AgentConfig, vnc_port: u16) -> Result<Self> {
        let cert_dir = PathBuf::from("/etc/urc/tls");
        fs::create_dir_all(&cert_dir).ok();

        let cert_path = cert_dir.join("agent.crt");
        let key_path = cert_dir.join("agent.key");

        if !cert_path.exists() || !key_path.exists() {
            generate_self_signed(&cert_path, &key_path)?;
            info!(path = %cert_dir.display(), "generated self-signed TLS certificate");
        }

        Ok(Self {
            listen_port: config.listen_tls_port,
            vnc_port,
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
            port = self.listen_port,
            vnc = self.vnc_port,
            "TLS tunnel listening (encrypted VNC)"
        );

        loop {
            let (client, addr) = listener.accept().await?;
            let acceptor = acceptor.clone();
            let vnc_port = self.vnc_port;
            tokio::spawn(async move {
                if let Err(e) = pipe_tls_client(client, acceptor, vnc_port).await {
                    tracing::debug!(%addr, error = %e, "TLS client session ended");
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

async fn pipe_tls_client(
    client: TcpStream,
    acceptor: TlsAcceptor,
    vnc_port: u16,
) -> Result<()> {
    let mut tls = acceptor.accept(client).await?;
    let mut vnc = TcpStream::connect(("127.0.0.1", vnc_port)).await?;

    let (mut tls_read, mut tls_write) = tokio::io::split(tls);
    let (mut vnc_read, mut vnc_write) = vnc.into_split();

    let c1 = tokio::io::copy(&mut tls_read, &mut vnc_write);
    let c2 = tokio::io::copy(&mut vnc_read, &mut tls_write);
    tokio::try_join!(c1, c2)?;
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
