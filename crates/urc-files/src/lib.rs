//! HTTP file transfer service for Ubuntu Remote Control.

use anyhow::Result;
use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::path::{Component, Path as StdPath, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Clone)]
struct FilesState {
    root: PathBuf,
}

#[derive(Serialize)]
struct ListEntry {
    name: String,
    is_dir: bool,
    size: u64,
}

pub async fn spawn_files_server(
    root: PathBuf,
    bind: &str,
    port: u16,
) -> Result<JoinHandle<()>> {
    let state = Arc::new(FilesState { root });

    let app = Router::new()
        .route("/api/list/{*path}", get(list_dir))
        .route("/api/download/{*path}", get(download_file))
        .route("/api/upload/{*path}", post(upload_file))
        .route("/api/health", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("{bind}:{port}");
    let listener = TcpListener::bind(&addr).await?;
    info!(%addr, "urc-files service listening");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    Ok(handle)
}

fn safe_path(root: &StdPath, rel: &str) -> Result<PathBuf, StatusCode> {
    let rel = rel.trim_start_matches('/');
    let mut path = root.to_path_buf();
    for part in StdPath::new(rel).components() {
        match part {
            Component::Normal(p) => path.push(p),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(StatusCode::BAD_REQUEST);
            }
            Component::CurDir => {}
        }
    }
    if !path.starts_with(root) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(path)
}

async fn list_dir(
    State(state): State<Arc<FilesState>>,
    Path(path): Path<String>,
) -> Result<Json<Vec<ListEntry>>, StatusCode> {
    let dir = safe_path(&state.root, &path)?;
    if !dir.is_dir() {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&dir).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    while let Some(entry) = read_dir.next_entry().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        let meta = entry.metadata().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        entries.push(ListEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: meta.is_dir(),
            size: meta.len(),
        });
    }
    Ok(Json(entries))
}

async fn download_file(
    State(state): State<Arc<FilesState>>,
    Path(path): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let file = safe_path(&state.root, &path)?;
    if !file.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }
    let data = tokio::fs::read(&file).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(data)
}

async fn upload_file(
    State(state): State<Arc<FilesState>>,
    Path(path): Path<String>,
    mut multipart: Multipart,
) -> Result<StatusCode, StatusCode> {
    let dest = safe_path(&state.root, &path)?;
    while let Some(field) = multipart.next_field().await.map_err(|_| StatusCode::BAD_REQUEST)? {
        let data = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::write(&dest, &data)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(StatusCode::CREATED);
    }
    Err(StatusCode::BAD_REQUEST)
}
