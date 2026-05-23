//! HTTP file transfer service for Ubuntu Remote Control.
//!
//! Exposes a small REST API as an `axum::Router` so callers (today: `urc-web`)
//! can mount it inside a larger application.

use anyhow::Result;
use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::io::Cursor;
use std::path::{Component, Path as StdPath, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use walkdir::WalkDir;

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

/// Build the file-API router rooted at `root`. Callers mount this under any prefix.
pub fn files_router(root: PathBuf) -> Router {
    let state = Arc::new(FilesState { root });
    Router::new()
        .route("/list", get(list_root))
        .route("/list/{*path}", get(list_dir))
        .route("/download/{*path}", get(download_file))
        .route("/download-zip", get(download_zip_root))
        .route("/download-zip/{*path}", get(download_zip))
        .route("/upload/{*path}", post(upload_file))
        .route("/health", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Stand-alone bind — kept as a convenience for tests; production wires the router
/// directly into urc-web on the unified port.
pub async fn spawn_files_server(
    root: PathBuf,
    bind: &str,
    port: u16,
) -> Result<JoinHandle<()>> {
    let app = Router::new().nest("/api", files_router(root));
    let addr = format!("{bind}:{port}");
    let listener = TcpListener::bind(&addr).await?;
    info!(%addr, "urc-files standalone listening");
    Ok(tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    }))
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

async fn list_root(
    State(state): State<Arc<FilesState>>,
) -> Result<Json<Vec<ListEntry>>, StatusCode> {
    list_path(&state, "").await
}

async fn list_dir(
    State(state): State<Arc<FilesState>>,
    Path(path): Path<String>,
) -> Result<Json<Vec<ListEntry>>, StatusCode> {
    list_path(&state, &path).await
}

async fn list_path(
    state: &FilesState,
    rel: &str,
) -> Result<Json<Vec<ListEntry>>, StatusCode> {
    let dir = safe_path(&state.root, rel)?;
    if !dir.is_dir() {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&dir)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        let meta = entry
            .metadata()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        entries.push(ListEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: meta.is_dir(),
            size: meta.len(),
        });
    }
    entries.sort_by(|a, b| (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase())));
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
    let data = tokio::fs::read(&file)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(data)
}

async fn download_zip_root(
    State(state): State<Arc<FilesState>>,
) -> Result<Response, StatusCode> {
    build_zip_response(&state.root, "remote-root".to_string()).await
}

async fn download_zip(
    State(state): State<Arc<FilesState>>,
    Path(path): Path<String>,
) -> Result<Response, StatusCode> {
    let dir = safe_path(&state.root, &path)?;
    if !dir.is_dir() {
        return Err(StatusCode::NOT_FOUND);
    }
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "folder".to_string());
    build_zip_response(&dir, name).await
}

/// Build a zip archive of `dir` on a blocking thread (the zip crate is sync),
/// then hand the bytes back as a downloadable response. Keep this in memory
/// for simplicity — streaming would require a custom AsyncWrite bridge.
async fn build_zip_response(dir: &StdPath, archive_name: String) -> Result<Response, StatusCode> {
    let dir = dir.to_path_buf();
    let bytes = tokio::task::spawn_blocking(move || zip_dir(&dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!(error = %e, "zip build failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let filename = format!("{archive_name}.zip");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                Box::leak(format!("attachment; filename=\"{filename}\"").into_boxed_str()),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

fn zip_dir(root: &StdPath) -> anyhow::Result<Vec<u8>> {
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let mut buf = Cursor::new(Vec::new());
    let mut zw = ZipWriter::new(&mut buf);
    let opts: SimpleFileOptions =
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if entry.file_type().is_dir() {
            zw.add_directory(format!("{rel_str}/"), opts)?;
        } else if entry.file_type().is_file() {
            zw.start_file(rel_str, opts)?;
            let mut f = std::fs::File::open(path)?;
            std::io::copy(&mut f, &mut zw)?;
        } else {
            // Skip symlinks / sockets / devices to keep archives portable.
            continue;
        }
    }
    let _ = zw.finish()?;
    Ok(buf.into_inner())
}

async fn upload_file(
    State(state): State<Arc<FilesState>>,
    Path(path): Path<String>,
    mut multipart: Multipart,
) -> Result<StatusCode, StatusCode> {
    let dest = safe_path(&state.root, &path)?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
    {
        let data = field
            .bytes()
            .await
            .map_err(|_| StatusCode::BAD_REQUEST)?;
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
