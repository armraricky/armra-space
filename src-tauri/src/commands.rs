use crate::db::{self, PinnedFile};
use crate::mount::{self, MountStatus, SharedMountState};
use crate::s3client::{self, S3Config, S3Entry};
use crate::sync::{self, SharedSyncProgress, SyncProgress};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;
use uuid::Uuid;

pub struct AppState {
    pub db_path: PathBuf,
    pub cache_dir: Mutex<PathBuf>,   // mutable — user can change it
    pub cache_max_mb: Mutex<u64>,    // 0 = unlimited
    pub config_dir: PathBuf,
    pub s3_config: Mutex<Option<S3Config>>,
    pub mount_state: SharedMountState,
    pub sync_progress: SharedSyncProgress,
    // ARMRA Quest control-plane integration.
    pub quest_base: String,                                  // e.g. https://armra.quest
    pub active_filespace: Mutex<Option<ActiveFilespace>>,    // currently-mounted scope
    pub pending_login: Mutex<Option<crate::auth::PendingPkce>>, // in-flight PKCE login
}

/// The filespace currently mounted, with its STS expiry so the UI can refresh
/// before the short-lived credentials lapse. `expiration` is None in static
/// mode (long-lived key — nothing to refresh).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ActiveFilespace {
    pub id: String,
    pub name: String,
    pub role: String,
    pub remote_path: String,
    pub expiration: Option<i64>, // epoch ms; None = non-expiring (static mode)
    pub mode: String,            // assume-role | federation | static
}

/// A filespace the signed-in user may mount (from GET /api/space/filespaces).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Filespace {
    pub id: String,
    pub name: String,
    pub bucket: String,
    pub prefix: String,
    pub region: Option<String>,
    pub role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilespacesResp {
    filespaces: Vec<Filespace>,
}

/// Scoped credentials from POST /api/space/sts (camelCase from the JSON API).
/// session_token/expiration are null in static mode (credential ladder rung 3).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StsResp {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    expiration: Option<i64>,
    bucket: String,
    prefix: String,
    region: String,
    remote_path: String,
    endpoint: Option<String>,
    role: String,
    name: String,
    filespace_id: String,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CacheConfig {
    pub path: String,
    pub max_mb: u64,
    pub used_mb: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MountStatusResponse {
    pub status: MountStatus,
    pub mount_point: Option<String>,
    pub error: Option<String>,
}

// ── Config ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn save_s3_config(
    state: State<'_, AppState>,
    config: S3Config,
) -> Result<(), String> {
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    db::set_config(&conn, "bucket", &config.bucket).map_err(|e| e.to_string())?;
    db::set_config(&conn, "region", &config.region).map_err(|e| e.to_string())?;
    db::set_config(&conn, "access_key", &config.access_key).map_err(|e| e.to_string())?;
    db::set_config(&conn, "secret_key", &config.secret_key).map_err(|e| e.to_string())?;
    if let Some(ep) = &config.endpoint {
        db::set_config(&conn, "endpoint", ep).map_err(|e| e.to_string())?;
    }
    if let Some(prefix) = &config.prefix {
        db::set_config(&conn, "prefix", prefix).map_err(|e| e.to_string())?;
    }
    *state.s3_config.lock().unwrap() = Some(config);
    Ok(())
}

#[tauri::command]
pub async fn load_s3_config(state: State<'_, AppState>) -> Result<Option<S3Config>, String> {
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    let bucket = db::get_config(&conn, "bucket").map_err(|e| e.to_string())?;
    let Some(bucket) = bucket else {
        return Ok(None);
    };
    let config = S3Config {
        bucket,
        region: db::get_config(&conn, "region")
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "us-east-1".into()),
        access_key: db::get_config(&conn, "access_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default(),
        secret_key: db::get_config(&conn, "secret_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default(),
        endpoint: db::get_config(&conn, "endpoint").map_err(|e| e.to_string())?,
        prefix: db::get_config(&conn, "prefix").map_err(|e| e.to_string())?,
        session_token: None,
    };
    // Manual creds are not a filespace — clear any stale role so a later
    // mount's read-only decision can't inherit it.
    *state.active_filespace.lock().unwrap() = None;
    *state.s3_config.lock().unwrap() = Some(config.clone());
    Ok(Some(config))
}

// ── Mount ──────────────────────────────────────────────────────────────────

/// The rclone remote target: "bucket" or "bucket/prefix" for a filespace scope.
fn remote_path_for(cfg: &S3Config) -> String {
    match &cfg.prefix {
        Some(p) if !p.trim_matches('/').is_empty() => {
            format!("{}/{}", cfg.bucket, p.trim_matches('/'))
        }
        _ => cfg.bucket.clone(),
    }
}

/// Mount whatever is in `s3_config` (manual creds or STS-scoped filespace creds).
/// If something is already mounted, it's unmounted first — so this doubles as the
/// refresh path when re-minting STS credentials before expiry.
async fn mount_current(state: &AppState) -> Result<MountStatusResponse, String> {
    let cfg = state
        .s3_config
        .lock()
        .unwrap()
        .clone()
        .ok_or("No filespace selected")?;

    // Tear down any existing mount first (refresh-safe).
    {
        let mut ms = state.mount_state.lock().await;
        if ms.status == MountStatus::Mounted {
            let _ = mount::kill_mount(&mut ms).await;
        }
        ms.status = MountStatus::Mounting;
    }

    let config_path = match mount::write_rclone_config(
        &state.config_dir,
        &cfg.region,
        &cfg.access_key,
        &cfg.secret_key,
        cfg.session_token.as_deref(),
        cfg.endpoint.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => {
            // Reset state — otherwise status pollers see 'mounting' forever.
            let mut ms = state.mount_state.lock().await;
            ms.status = MountStatus::Error;
            ms.error = Some(e.to_string());
            return Err(e.to_string());
        }
    };

    let remote_path = remote_path_for(&cfg);
    let rclone_bin = match mount::resolve_rclone_binary(&state.config_dir) {
        Ok(b) => b,
        Err(e) => {
            let mut ms = state.mount_state.lock().await;
            ms.status = MountStatus::Error;
            ms.error = Some(e.to_string());
            return Err(e.to_string());
        }
    };
    let cache_dir = state.cache_dir.lock().unwrap().clone();
    // One lock: the active filespace gives both the mount subfolder name (so
    // the drive mounts INSIDE the branded ~/ARMRA Space folder, named by
    // filespace) and the role (viewers mount read-only — the only write guard
    // in static-credential mode).
    let (subdir, read_only) = {
        let af = state.active_filespace.lock().unwrap();
        match af.as_ref() {
            Some(a) => (a.name.clone(), a.role == "viewer"),
            None => (cfg.bucket.clone(), false), // legacy manual path
        }
    };
    let mount_point = mount::mount_point_for(&subdir);

    match mount::spawn_mount(&rclone_bin, &config_path, &remote_path, &mount_point, &cache_dir, read_only, &subdir).await {
        Ok(mut child) => {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            // rclone runs in the foreground (--daemon=false), so if the mount
            // failed (no macFUSE, bad/expired creds) the process has already
            // exited. Probe before declaring success, and surface its stderr.
            match child.try_wait() {
                Ok(Some(_status)) => {
                    let mut msg = String::new();
                    if let Some(mut err) = child.stderr.take() {
                        use tokio::io::AsyncReadExt;
                        let _ = err.read_to_string(&mut msg).await;
                    }
                    let msg = if msg.trim().is_empty() {
                        "The mount helper exited before the drive was ready. Try again, or reinstall the latest ARMRA Space.".to_string()
                    } else {
                        msg.trim().to_string()
                    };
                    let mut ms = state.mount_state.lock().await;
                    ms.status = MountStatus::Error;
                    ms.error = Some(msg.clone());
                    ms.child = None;
                    ms.mount_point = None;
                    Err(msg)
                }
                _ => {
                    let mut ms = state.mount_state.lock().await;
                    ms.child = Some(child);
                    ms.status = MountStatus::Mounted;
                    ms.mount_point = Some(mount_point.clone());
                    ms.error = None;
                    Ok(MountStatusResponse {
                        status: MountStatus::Mounted,
                        mount_point: Some(mount_point.to_string_lossy().into_owned()),
                        error: None,
                    })
                }
            }
        }
        Err(e) => {
            let mut ms = state.mount_state.lock().await;
            ms.status = MountStatus::Error;
            ms.error = Some(e.to_string());
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn mount_bucket(state: State<'_, AppState>) -> Result<MountStatusResponse, String> {
    if state.s3_config.lock().unwrap().is_none() {
        return Err("No filespace selected".into());
    }
    mount_current(state.inner()).await
}

// ── Filespaces (ARMRA Quest) ─────────────────────────────────────────────────

/// List the filespaces the signed-in user may mount.
#[tauri::command]
pub async fn list_filespaces(state: State<'_, AppState>) -> Result<Vec<Filespace>, String> {
    let token = crate::auth::load_token().ok_or("Not signed in")?;
    let base = state.quest_base.clone();
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/space/filespaces", base))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        return Err(body.get("error").and_then(|v| v.as_str()).unwrap_or("Failed to list filespaces").to_string());
    }
    let parsed: FilespacesResp = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parsed.filespaces)
}

/// Mint scoped credentials for a filespace and make it the active scope. This
/// enables browsing (via the S3 API) immediately; mounting as a Finder drive is
/// a separate step (mount_bucket) so browsing never requires macFUSE. Re-calling
/// this refreshes the short-lived STS credentials before they expire.
#[tauri::command]
pub async fn open_filespace(
    state: State<'_, AppState>,
    filespace_id: String,
) -> Result<ActiveFilespace, String> {
    let token = crate::auth::load_token().ok_or("Not signed in")?;
    let base = state.quest_base.clone();
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/space/sts", base))
        .bearer_auth(token)
        .json(&serde_json::json!({ "filespaceId": filespace_id }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        return Err(body.get("error").and_then(|v| v.as_str()).unwrap_or("Failed to get credentials").to_string());
    }
    let sts: StsResp = resp.json().await.map_err(|e| e.to_string())?;

    let prefix_opt = if sts.prefix.trim_matches('/').is_empty() {
        None
    } else {
        Some(sts.prefix.clone())
    };
    let cfg = S3Config {
        bucket: sts.bucket.clone(),
        region: sts.region.clone(),
        access_key: sts.access_key_id.clone(),
        secret_key: sts.secret_access_key.clone(),
        endpoint: sts.endpoint.clone(),
        prefix: prefix_opt,
        session_token: sts.session_token.clone(),
    };
    let active = ActiveFilespace {
        id: sts.filespace_id.clone(),
        name: sts.name.clone(),
        role: sts.role.clone(),
        remote_path: sts.remote_path.clone(),
        expiration: sts.expiration,
        mode: sts.mode.clone().unwrap_or_else(|| "federation".into()),
    };
    // Hold both locks so the credentials and the role that scopes them update
    // as one unit — a racing mount can never pair new creds with a stale role.
    {
        let mut af = state.active_filespace.lock().unwrap();
        let mut sc = state.s3_config.lock().unwrap();
        *sc = Some(cfg);
        *af = Some(active.clone());
    }
    Ok(active)
}

/// The currently-mounted filespace (id/name/role + STS expiry), if any.
#[tauri::command]
pub async fn get_active_filespace(state: State<'_, AppState>) -> Result<Option<ActiveFilespace>, String> {
    Ok(state.active_filespace.lock().unwrap().clone())
}

#[tauri::command]
pub async fn unmount_bucket(state: State<'_, AppState>) -> Result<(), String> {
    let mut ms = state.mount_state.lock().await;
    mount::kill_mount(&mut ms).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_mount_status(state: State<'_, AppState>) -> Result<MountStatusResponse, String> {
    let ms = state.mount_state.lock().await;
    Ok(MountStatusResponse {
        status: ms.status.clone(),
        mount_point: ms.mount_point.as_ref().map(|p| p.to_string_lossy().into_owned()),
        error: ms.error.clone(),
    })
}

// ── Files ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_files(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<S3Entry>, String> {
    let cfg = state
        .s3_config
        .lock()
        .unwrap()
        .clone()
        .ok_or("No S3 config saved")?;

    let client = s3client::make_client(&cfg).await.map_err(|e| e.to_string())?;
    s3client::list_objects(&client, &cfg, &path)
        .await
        .map_err(|e| e.to_string())
}

// ── Pins ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn pin_file(
    state: State<'_, AppState>,
    s3_key: String,
    size: i64,
) -> Result<PinnedFile, String> {
    let cfg = state
        .s3_config
        .lock()
        .unwrap()
        .clone()
        .ok_or("No S3 config saved")?;

    let local_path = state.cache_dir.lock().unwrap().join("pins").join(&s3_key);

    let pin = PinnedFile {
        id: Uuid::new_v4().to_string(),
        s3_key: s3_key.clone(),
        bucket: cfg.bucket.clone(),
        local_path: local_path.to_string_lossy().into_owned(),
        size,
        last_synced: None,
        is_cached: false,
        etag: None,
    };

    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    db::upsert_pin(&conn, &pin).map_err(|e| e.to_string())?;
    Ok(pin)
}

#[tauri::command]
pub async fn unpin_file(state: State<'_, AppState>, s3_key: String) -> Result<(), String> {
    let cfg = state
        .s3_config
        .lock()
        .unwrap()
        .clone()
        .ok_or("No S3 config saved")?;

    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    db::delete_pin(&conn, &cfg.bucket, &s3_key).map_err(|e| e.to_string())?;

    let local_path = state.cache_dir.lock().unwrap().join("pins").join(&s3_key);
    let _ = tokio::fs::remove_file(&local_path).await;
    Ok(())
}

#[tauri::command]
pub async fn list_pins(state: State<'_, AppState>) -> Result<Vec<PinnedFile>, String> {
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    db::list_pins(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn start_sync(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let cfg = state
        .s3_config
        .lock()
        .unwrap()
        .clone()
        .ok_or("No S3 config saved")?;

    let db_path = state.db_path.clone();
    let max_bytes = *state.cache_max_mb.lock().unwrap() * 1024 * 1024;
    let progress = state.sync_progress.clone();

    tokio::spawn(async move {
        let _ = sync::sync_pins(db_path, cfg, max_bytes, progress, app_handle).await;
    });
    Ok(())
}

// ── Cache config ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_cache_config(state: State<'_, AppState>) -> Result<CacheConfig, String> {
    let path = state.cache_dir.lock().unwrap().to_string_lossy().into_owned();
    let max_mb = *state.cache_max_mb.lock().unwrap();
    let used_bytes = sync::disk_usage_bytes(state.cache_dir.lock().unwrap().join("pins"));
    Ok(CacheConfig {
        path,
        max_mb,
        used_mb: used_bytes as f64 / 1_048_576.0,
    })
}

#[tauri::command]
pub async fn set_cache_config(
    state: State<'_, AppState>,
    path: String,
    max_mb: u64,
) -> Result<(), String> {
    let new_dir = PathBuf::from(&path);
    std::fs::create_dir_all(new_dir.join("pins")).map_err(|e| e.to_string())?;

    let old_dir = {
        let mut guard = state.cache_dir.lock().unwrap();
        let old = guard.clone();
        *guard = new_dir.clone();
        old
    };
    *state.cache_max_mb.lock().unwrap() = max_mb;

    // Persist to DB
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    db::set_config(&conn, "cache_path", &path).map_err(|e| e.to_string())?;
    db::set_config(&conn, "cache_max_mb", &max_mb.to_string()).map_err(|e| e.to_string())?;

    // If path changed, invalidate all cached pins so they re-download to new location
    if old_dir != new_dir {
        let pins = db::list_pins(&conn).map_err(|e| e.to_string())?;
        for pin in pins {
            let new_local = new_dir.join("pins").join(&pin.s3_key);
            let updated = db::PinnedFile {
                local_path: new_local.to_string_lossy().into_owned(),
                is_cached: false,
                last_synced: None,
                etag: None,
                ..pin
            };
            db::upsert_pin(&conn, &updated).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn reveal_cache_dir(state: State<'_, AppState>) -> Result<(), String> {
    let dir = state.cache_dir.lock().unwrap().join("pins");
    std::fs::create_dir_all(&dir).ok();

    #[cfg(target_os = "macos")]
    tokio::process::Command::new("open")
        .arg(dir.to_str().unwrap())
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    tokio::process::Command::new("explorer")
        .arg(dir.to_str().unwrap())
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn get_sync_progress(state: State<'_, AppState>) -> Result<SyncProgress, String> {
    Ok(state.sync_progress.lock().unwrap().clone())
}

#[tauri::command]
pub async fn open_in_finder(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    tokio::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    tokio::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn reveal_mount_point(state: State<'_, AppState>) -> Result<(), String> {
    let mp = state
        .mount_state
        .lock()
        .await
        .mount_point
        .clone()
        .ok_or("Not mounted")?;

    #[cfg(target_os = "macos")]
    tokio::process::Command::new("open")
        .arg(mp.to_str().unwrap())
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    tokio::process::Command::new("explorer")
        .arg(mp.to_str().unwrap())
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}
