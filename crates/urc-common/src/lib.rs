//! Shared protocol types for Ubuntu Remote Control.

pub mod tailscale;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_COORDINATOR_PORT: u16 = 21150;
pub const DEFAULT_RELAY_VNC_PORT: u16 = 15900;
pub const DEFAULT_FILES_PORT: u16 = 15901;
pub const DEFAULT_TLS_LISTEN_PORT: u16 = 15900;
/// External TLS port for the unified web UI (noVNC + files + future panels).
pub const DEFAULT_WEB_TLS_PORT: u16 = 15901;
/// Internal localhost port the urc-web server binds on; only the TLS tunnel reaches it.
pub const DEFAULT_WEB_INTERNAL_PORT: u16 = 16080;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    Register {
        host_id: String,
        token: String,
        tailscale_ip: Option<String>,
        vnc_local_port: u16,
    },
    Heartbeat {
        host_id: String,
    },
    TunnelReady {
        session_id: Uuid,
        target: TunnelTarget,
    },
    TunnelData {
        session_id: Uuid,
        target: TunnelTarget,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    TunnelClose {
        session_id: Uuid,
        target: TunnelTarget,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TunnelTarget {
    Vnc,
    Files,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Connect { host_id: String, token: String },
    Disconnect { session_id: Uuid },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoordinatorMessage {
    Registered {
        host_id: String,
    },
    ConnectOk {
        session_id: Uuid,
        relay_mode: RelayMode,
    },
    ConnectErr {
        reason: String,
    },
    RelayHint {
        session_id: Uuid,
        coordinator_addr: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelayMode {
    Direct,
    Relayed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub host_id: Option<String>,
    pub token: Option<String>,
    pub coordinator_url: String,
    pub listen_tls_port: u16,
    #[serde(default = "default_web_tls_port")]
    pub listen_web_tls_port: u16,
    pub vnc_password_file: Option<String>,
    pub encryption: EncryptionMode,
    pub insecure: bool,
    pub tailscale: TailscaleConfig,
    pub files_root: Option<String>,
    pub backend: BackendPreference,
}

fn default_web_tls_port() -> u16 {
    DEFAULT_WEB_TLS_PORT
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionMode {
    #[default]
    Tls,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TailscaleConfig {
    pub enabled: bool,
    pub prefer_direct: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackendPreference {
    #[default]
    Auto,
    X11,
    Gnome,
    Wayvnc,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            host_id: None,
            token: None,
            coordinator_url: format!("wss://127.0.0.1:{}", DEFAULT_COORDINATOR_PORT),
            listen_tls_port: DEFAULT_TLS_LISTEN_PORT,
            listen_web_tls_port: DEFAULT_WEB_TLS_PORT,
            vnc_password_file: Some("/etc/urc/vncpasswd".into()),
            encryption: EncryptionMode::Tls,
            insecure: false,
            tailscale: TailscaleConfig::default(),
            files_root: Some("/home".into()),
            backend: BackendPreference::Auto,
        }
    }
}

pub fn parse_ws_message<T: for<'de> Deserialize<'de>>(text: &str) -> anyhow::Result<T> {
    Ok(serde_json::from_str(text)?)
}

pub fn to_ws_message<T: Serialize>(msg: &T) -> anyhow::Result<String> {
    Ok(serde_json::to_string(msg)?)
}

mod serde_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        STANDARD.encode(bytes).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}
