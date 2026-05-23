//! WebSocket relay hub — bridges agent and client tunnel legs.

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use uuid::Uuid;

struct Session {
    agent_tx: Option<mpsc::UnboundedSender<Message>>,
    client_tx: Option<mpsc::UnboundedSender<Message>>,
}

pub struct RelayHub {
    sessions: Arc<RwLock<HashMap<Uuid, Arc<Mutex<Session>>>>>,
}

impl RelayHub {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create_session(&self, id: Uuid) {
        self.sessions.write().await.insert(
            id,
            Arc::new(Mutex::new(Session {
                agent_tx: None,
                client_tx: None,
            })),
        );
    }

    pub async fn attach_agent(&self, id: Uuid, socket: WebSocket) {
        self.bridge_leg(id, socket, true).await;
    }

    pub async fn attach_client(&self, id: Uuid, socket: WebSocket) {
        self.bridge_leg(id, socket, false).await;
    }

    async fn bridge_leg(&self, id: Uuid, socket: WebSocket, is_agent: bool) {
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::unbounded_channel();

        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(&id) {
                let mut s = session.lock().await;
                if is_agent {
                    s.agent_tx = Some(tx);
                } else {
                    s.client_tx = Some(tx);
                }
            }
        }

        let sessions = Arc::clone(&self.sessions);
        let forward = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        while let Some(Ok(msg)) = stream.next().await {
            let peer_tx = {
                let sessions = sessions.read().await;
                let Some(session) = sessions.get(&id) else {
                    break;
                };
                let s = session.lock().await;
                if is_agent {
                    s.client_tx.clone()
                } else {
                    s.agent_tx.clone()
                }
            };
            if let Some(peer) = peer_tx {
                let _ = peer.send(msg);
            }
        }

        forward.abort();
        self.sessions.write().await.remove(&id);
    }
}
