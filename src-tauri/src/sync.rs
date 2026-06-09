use crate::db::{self, PinnedFile};
use crate::s3client::{self, S3Config};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::Emitter;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncProgress {
    pub total: usize,
    pub done: usize,
    pub current_key: Option<String>,
    pub errors: Vec<String>,
}

pub type SharedSyncProgress = Arc<Mutex<SyncProgress>>;

pub fn new_progress() -> SharedSyncProgress {
    Arc::new(Mutex::new(SyncProgress {
        total: 0,
        done: 0,
        current_key: None,
        errors: Vec::new(),
    }))
}

/// Download all pinned files that aren't cached yet.
/// `max_bytes` — 0 means unlimited.
pub async fn sync_pins(
    db_path: PathBuf,
    s3_cfg: S3Config,
    max_bytes: u64,
    progress: SharedSyncProgress,
    app_handle: tauri::AppHandle,
) -> Result<()> {
    let conn = db::open(&db_path)?;
    let pins = db::list_pins(&conn)?;
    drop(conn);

    let pending: Vec<PinnedFile> = pins.into_iter().filter(|p| !p.is_cached).collect();

    {
        let mut prog = progress.lock().unwrap();
        prog.total = pending.len();
        prog.done = 0;
        prog.errors.clear();
    }

    let client = s3client::make_client(&s3_cfg).await?;
    let mut bytes_written: u64 = 0;

    for pin in &pending {
        // Enforce size limit before each download
        if max_bytes > 0 {
            let current_usage = disk_usage_bytes(PathBuf::from(&pin.local_path)
                .parent()
                .unwrap_or(std::path::Path::new("/"))
                .parent()
                .unwrap_or(std::path::Path::new("/")));
            if current_usage + bytes_written + pin.size.max(0) as u64 > max_bytes {
                let mut prog = progress.lock().unwrap();
                prog.errors.push(format!(
                    "{}: skipped — cache limit reached",
                    pin.s3_key
                ));
                prog.done += 1;
                let _ = app_handle.emit("sync-progress", prog.clone());
                continue;
            }
        }

        {
            let mut prog = progress.lock().unwrap();
            prog.current_key = Some(pin.s3_key.clone());
        }
        let _ = app_handle.emit("sync-progress", progress.lock().unwrap().clone());

        let dest = PathBuf::from(&pin.local_path);
        match s3client::download_object(&client, &pin.bucket, &pin.s3_key, &dest).await {
            Ok(etag) => {
                bytes_written += pin.size.max(0) as u64;
                let conn = db::open(&db_path)?;
                let _ = db::mark_cached(&conn, &pin.bucket, &pin.s3_key, Some(&etag));
            }
            Err(e) => {
                let mut prog = progress.lock().unwrap();
                prog.errors.push(format!("{}: {}", pin.s3_key, e));
            }
        }

        {
            let mut prog = progress.lock().unwrap();
            prog.done += 1;
        }
        let _ = app_handle.emit("sync-progress", progress.lock().unwrap().clone());
    }

    {
        let mut prog = progress.lock().unwrap();
        prog.current_key = None;
    }
    let _ = app_handle.emit("sync-progress", progress.lock().unwrap().clone());

    Ok(())
}

/// Recursively sum file sizes under `dir`.
pub fn disk_usage_bytes(dir: impl AsRef<std::path::Path>) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}
