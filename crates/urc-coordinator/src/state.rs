//! Coordinator application state.

use axum::extract::ws::Message;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

#[derive(Clone, Debug)]
pub struct AgentRecord {
    pub host_id: String,
    pub tailscale_ip: Option<String>,
    pub vnc_local_port: u16,
    // Set on registration but not yet surfaced anywhere (no files-transfer hint
    // exists to mirror the vnc_local_port direct-connect hint). See follow-up task.
    #[allow(dead_code)]
    pub files_local_port: u16,
    pub last_seen: DateTime<Utc>,
    pub control_tx: mpsc::UnboundedSender<Message>,
}

#[derive(Clone, Serialize)]
pub struct HostSummary {
    pub host_id: String,
    pub tailscale_ip: Option<String>,
    pub online: bool,
}

pub struct AppState {
    secret: String,
    agents: RwLock<HashMap<String, AgentRecord>>,
    pub relay: Arc<super::relay::RelayHub>,
}

impl AppState {
    pub fn new(secret: String) -> Self {
        Self {
            secret,
            agents: RwLock::new(HashMap::new()),
            relay: Arc::new(super::relay::RelayHub::new()),
        }
    }

    pub fn verify_token(&self, token: &str) -> bool {
        token == self.secret || (!self.secret.is_empty() && token == self.secret)
    }

    pub async fn register_agent(
        &self,
        host_id: String,
        tailscale_ip: Option<String>,
        vnc_local_port: u16,
        files_local_port: u16,
        control_tx: mpsc::UnboundedSender<Message>,
    ) {
        let record = AgentRecord {
            host_id: host_id.clone(),
            tailscale_ip,
            vnc_local_port,
            files_local_port,
            last_seen: Utc::now(),
            control_tx,
        };
        self.agents.write().await.insert(host_id, record);
    }

    pub async fn touch_agent(&self, host_id: &str) {
        if let Some(a) = self.agents.write().await.get_mut(host_id) {
            a.last_seen = Utc::now();
        }
    }

    pub async fn unregister_agent(&self, host_id: &str) {
        self.agents.write().await.remove(host_id);
    }

    pub async fn get_agent(&self, host_id: &str) -> Option<AgentRecord> {
        self.agents.read().await.get(host_id).cloned()
    }

    pub async fn agent_control_tx(&self, host_id: &str) -> Option<mpsc::UnboundedSender<Message>> {
        self.agents
            .read()
            .await
            .get(host_id)
            .map(|a| a.control_tx.clone())
    }

    pub async fn list_hosts(&self) -> Vec<HostSummary> {
        self.agents
            .read()
            .await
            .values()
            .map(|a| HostSummary {
                host_id: a.host_id.clone(),
                tailscale_ip: a.tailscale_ip.clone(),
                online: true,
            })
            .collect()
    }
}
