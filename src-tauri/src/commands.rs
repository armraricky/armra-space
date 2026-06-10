use crate::db::{self, PinnedFile};
use crate::mount::{self, MountStatus, MountState, Mounts};
use crate::s3client::{self, S3Config, S3Entry};
use crate::sync::{self, SharedSyncProgress, SyncProgress};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;
use uuid::Uuid;

pub struct AppState {
    pub db_path: PathBuf,
    pub cache_dir: Mutex<PathBuf>,   // mutable — user can change it
    pub cache_max_mb: Mutex<u64>,    // 0 = unlimited
    pub config_dir: PathBuf,
    // The SELECTED filespace — drives browsing, pinning, sync, list_files. This
    // is distinct from what's MOUNTED: selecting a filespace loads its creds for
    // browsing without necessarily mounting it.
    pub s3_config: Mutex<Option<S3Config>>,
    pub active_filespace: Mutex<Option<ActiveFilespace>>,
    // All currently-mounted filespaces (multi-mount), keyed by filespace id.
    pub mounts: Mounts,
    // Per-filespace creds cache, so we can (re)mount or refresh a specific
    // filespace's STS without disturbing the selected/browsed one.
    pub fs_configs: Mutex<HashMap<String, (S3Config, ActiveFilespace)>>,
    pub sync_progress: SharedSyncProgress,
    // ARMRA Quest control-plane integration.
    pub quest_base: String,                                  // e.g. https://armra.quest
    pub pending_login: Mutex<Option<crate::auth::PendingPkce>>, // in-flight PKCE login
}

/// One mounted filespace, for the UI's connected list.
#[derive(Debug, Serialize, Clone)]
pub struct MountInfo {
    pub id: String,
    pub name: String,
    pub status: MountStatus,
    pub mount_point: Option<String>,
    pub error: Option<String>,
    pub expiration: Option<i64>,
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
    #[serde(default)]
    accelerate: Option<bool>,
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
        accelerate: false,
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

fn mount_info(id: &str, ms: &MountState) -> MountInfo {
    MountInfo {
        id: id.to_string(),
        name: ms.name.clone(),
        status: ms.status.clone(),
        mount_point: ms.mount_point.as_ref().map(|p| p.to_string_lossy().into_owned()),
        error: ms.error.clone(),
        expiration: ms.expiration,
    }
}

/// Fetch scoped STS creds for a filespace from Quest. Pure — does not touch
/// app state — so it's shared by open (select) and refresh (re-mint) paths.
async fn fetch_sts(state: &AppState, filespace_id: &str) -> Result<(S3Config, ActiveFilespace), String> {
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
    let prefix_opt = if sts.prefix.trim_matches('/').is_empty() { None } else { Some(sts.prefix.clone()) };
    let cfg = S3Config {
        bucket: sts.bucket.clone(),
        region: sts.region.clone(),
        access_key: sts.access_key_id.clone(),
        secret_key: sts.secret_access_key.clone(),
        endpoint: sts.endpoint.clone(),
        prefix: prefix_opt,
        session_token: sts.session_token.clone(),
        accelerate: sts.accelerate.unwrap_or(false),
    };
    let active = ActiveFilespace {
        id: sts.filespace_id.clone(),
        name: sts.name.clone(),
        role: sts.role.clone(),
        remote_path: sts.remote_path.clone(),
        expiration: sts.expiration,
        mode: sts.mode.clone().unwrap_or_else(|| "federation".into()),
    };
    Ok((cfg, active))
}

/// Mark a mount errored and return the message (so callers can `return Err(...)`).
async fn fail_mount(state: &AppState, id: &str, msg: String) -> String {
    let mut mounts = state.mounts.lock().await;
    if let Some(ms) = mounts.get_mut(id) {
        ms.status = MountStatus::Error;
        ms.error = Some(msg.clone());
        ms.child = None;
        ms.mount_point = None;
    }
    msg
}

/// Mount one filespace by id using creds cached in `fs_configs`, ADDING it to
/// the mounted set without disturbing other live mounts. Re-mounting the same
/// id (e.g. after an STS refresh) tears down just that one first.
async fn do_mount(state: &AppState, id: &str) -> Result<MountInfo, String> {
    let (cfg, active) = state
        .fs_configs
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or("Filespace creds not loaded — open it first")?;

    // Tear down any existing mount for THIS id, then reserve a private rc port.
    let rc_port = {
        let mut mounts = state.mounts.lock().await;
        if let Some(mut old) = mounts.remove(id) {
            let _ = mount::kill_mount(&mut old).await;
        }
        let used: Vec<u16> = mounts.values().map(|m| m.rc_port).collect();
        let port = mount::pick_rc_port(&used);
        let mut ms = MountState::new();
        ms.status = MountStatus::Mounting;
        ms.rc_port = port;
        ms.name = active.name.clone();
        ms.expiration = active.expiration;
        mounts.insert(id.to_string(), ms);
        port
    };

    let config_path = match mount::write_rclone_config(
        &state.config_dir, id, &cfg.region, &cfg.access_key, &cfg.secret_key,
        cfg.session_token.as_deref(), cfg.endpoint.as_deref(), cfg.accelerate,
    ) {
        Ok(p) => p,
        Err(e) => return Err(fail_mount(state, id, e.to_string()).await),
    };
    let remote_path = remote_path_for(&cfg);
    let rclone_bin = match mount::resolve_rclone_binary(&state.config_dir) {
        Ok(b) => b,
        Err(e) => return Err(fail_mount(state, id, e.to_string()).await),
    };
    let cache_dir = state.cache_dir.lock().unwrap().clone();
    let cache_max_mb = *state.cache_max_mb.lock().unwrap();
    let subdir = active.name.clone();
    let read_only = active.role == "viewer";
    let mount_point = mount::mount_point_for(&subdir);

    // Detach a stale mount left at this path by a previous run/crash.
    if mount::is_path_mounted(&mount_point) {
        mount::force_unmount_stale(&mount_point).await;
    }

    match mount::spawn_mount(&rclone_bin, &config_path, &remote_path, &mount_point, &cache_dir, read_only, &subdir, cache_max_mb, rc_port).await {
        Ok(mut child) => {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            // Foreground rclone exits immediately if the mount failed; probe + surface stderr.
            match child.try_wait() {
                Ok(Some(_status)) => {
                    let mut msg = String::new();
                    if let Some(mut err) = child.stderr.take() {
                        use tokio::io::AsyncReadExt;
                        let _ = err.read_to_string(&mut msg).await;
                    }
                    let msg = if msg.trim().is_empty() {
                        "The mount helper exited before the drive was ready. Try again, or reinstall the latest ARMRA Space.".to_string()
                    } else { msg.trim().to_string() };
                    Err(fail_mount(state, id, msg).await)
                }
                _ => {
                    let mut mounts = state.mounts.lock().await;
                    match mounts.get_mut(id) {
                        Some(ms) => {
                            ms.child = Some(child);
                            ms.status = MountStatus::Mounted;
                            ms.mount_point = Some(mount_point.clone());
                            ms.error = None;
                            Ok(mount_info(id, ms))
                        }
                        None => {
                            // Raced with an unmount — clean up the orphan child.
                            let _ = child.kill().await;
                            Err("Mount was cancelled".into())
                        }
                    }
                }
            }
        }
        Err(e) => Err(fail_mount(state, id, e.to_string()).await),
    }
}

#[tauri::command]
pub async fn mount_bucket(state: State<'_, AppState>) -> Result<MountStatusResponse, String> {
    // Mount the SELECTED filespace (set by open_filespace), alongside any others.
    let id = state
        .active_filespace
        .lock()
        .unwrap()
        .as_ref()
        .map(|a| a.id.clone())
        .ok_or("No filespace selected")?;
    let info = do_mount(state.inner(), &id).await?;
    Ok(MountStatusResponse { status: info.status, mount_point: info.mount_point, error: info.error })
}

/// All currently-mounted filespaces, for the UI's connected list.
#[tauri::command]
pub async fn get_mounts(state: State<'_, AppState>) -> Result<Vec<MountInfo>, String> {
    let mounts = state.mounts.lock().await;
    Ok(mounts.iter().map(|(id, ms)| mount_info(id, ms)).collect())
}

/// Unmount one filespace (leaves any others mounted).
#[tauri::command]
pub async fn unmount_filespace(state: State<'_, AppState>, filespace_id: String) -> Result<(), String> {
    let mut mounts = state.mounts.lock().await;
    if let Some(mut ms) = mounts.remove(&filespace_id) {
        let _ = mount::kill_mount(&mut ms).await;
    }
    Ok(())
}

/// Re-mint STS creds for a mounted filespace and remount it with fresh creds —
/// used to refresh before expiry without changing what's being browsed.
#[tauri::command]
pub async fn refresh_filespace(state: State<'_, AppState>, filespace_id: String) -> Result<(), String> {
    let (cfg, active) = fetch_sts(state.inner(), &filespace_id).await?;
    state.fs_configs.lock().unwrap().insert(filespace_id.clone(), (cfg.clone(), active.clone()));
    // If this is also the SELECTED filespace, refresh the browse/pin/sync creds.
    {
        let is_selected = state.active_filespace.lock().unwrap().as_ref().map(|a| a.id == filespace_id).unwrap_or(false);
        if is_selected {
            *state.s3_config.lock().unwrap() = Some(cfg);
            *state.active_filespace.lock().unwrap() = Some(active);
        }
    }
    let is_mounted = state.mounts.lock().await.contains_key(&filespace_id);
    if is_mounted {
        do_mount(state.inner(), &filespace_id).await?;
    }
    Ok(())
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
    let (cfg, active) = fetch_sts(state.inner(), &filespace_id).await?;
    // Make this the SELECTED filespace (drives browse/pin/sync) and cache its
    // creds so it (or a refresh) can be mounted. Does NOT unmount anything.
    {
        let mut af = state.active_filespace.lock().unwrap();
        let mut sc = state.s3_config.lock().unwrap();
        *sc = Some(cfg.clone());
        *af = Some(active.clone());
    }
    state.fs_configs.lock().unwrap().insert(filespace_id.clone(), (cfg, active.clone()));
    Ok(active)
}

/// The currently-SELECTED filespace (id/name/role + STS expiry), if any.
#[tauri::command]
pub async fn get_active_filespace(state: State<'_, AppState>) -> Result<Option<ActiveFilespace>, String> {
    Ok(state.active_filespace.lock().unwrap().clone())
}

/// Unmount the SELECTED filespace (kept for compatibility; the UI uses
/// unmount_filespace for a specific one).
#[tauri::command]
pub async fn unmount_bucket(state: State<'_, AppState>) -> Result<(), String> {
    let id = state.active_filespace.lock().unwrap().as_ref().map(|a| a.id.clone());
    if let Some(id) = id {
        let mut mounts = state.mounts.lock().await;
        if let Some(mut ms) = mounts.remove(&id) {
            let _ = mount::kill_mount(&mut ms).await;
        }
    }
    Ok(())
}

/// Status of the SELECTED filespace's mount (compat shim; UI uses get_mounts).
#[tauri::command]
pub async fn get_mount_status(state: State<'_, AppState>) -> Result<MountStatusResponse, String> {
    let id = state.active_filespace.lock().unwrap().as_ref().map(|a| a.id.clone());
    let mounts = state.mounts.lock().await;
    let entry = id.as_ref().and_then(|i| mounts.get(i));
    Ok(MountStatusResponse {
        status: entry.map(|m| m.status.clone()).unwrap_or(MountStatus::Unmounted),
        mount_point: entry.and_then(|m| m.mount_point.as_ref().map(|p| p.to_string_lossy().into_owned())),
        error: entry.and_then(|m| m.error.clone()),
    })
}

// ── Files ──────────────────────────────────────────────────────────────────

fn listing_key(cfg: &S3Config, path: &str) -> String {
    format!("{}:{}|{}", cfg.bucket, cfg.prefix.as_deref().unwrap_or(""), path)
}

/// True for OS-generated junk files that should never appear in the file list.
fn is_junk_name(name: &str) -> bool {
    name == ".DS_Store"
        || name == "Thumbs.db"
        || name == ".localized"
        || name.starts_with("._")
        || name.starts_with(".Spotlight-V")
        || name.starts_with(".Trash")
        || name == ".fseventsd"
        || name == ".TemporaryItems"
        || name == ".apdisk"
}

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
    let mut entries = s3client::list_objects(&client, &cfg, &path)
        .await
        .map_err(|e| e.to_string())?;
    // Hide macOS/Windows junk that can linger in the bucket (the rclone mount
    // excludes these going forward, but older objects may still be present).
    entries.retain(|e| !is_junk_name(&e.name));
    // Cache the listing so the browse tab can paint instantly next time.
    if let Ok(conn) = db::open(&state.db_path) {
        if let Ok(json) = serde_json::to_string(&entries) {
            let _ = db::set_listing_cache(&conn, &listing_key(&cfg, &path), &json);
        }
    }
    Ok(entries)
}

/// Instant cached listing for a path (last-known tree). The UI shows this, then
/// calls list_files for the live refresh.
#[tauri::command]
pub async fn cached_listing(
    state: State<'_, AppState>,
    path: String,
) -> Result<Option<Vec<S3Entry>>, String> {
    let cfg = match state.s3_config.lock().unwrap().clone() { Some(c) => c, None => return Ok(None) };
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    match db::get_listing_cache(&conn, &listing_key(&cfg, &path)).map_err(|e| e.to_string())? {
        Some(json) => {
            let cached: Option<Vec<S3Entry>> = serde_json::from_str(&json).ok();
            Ok(cached.map(|mut v| { v.retain(|e| !is_junk_name(&e.name)); v }))
        }
        None => Ok(None),
    }
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

/// Pin an entire folder: recursively enumerate every file under it and pin each
/// one, so the whole folder is available offline. Returns how many files were
/// pinned. Kicks off a sync so the bytes start downloading immediately.
#[tauri::command]
pub async fn pin_folder(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
    path: String,
) -> Result<usize, String> {
    let cfg = state.s3_config.lock().unwrap().clone().ok_or("No S3 config saved")?;
    let client = s3client::make_client(&cfg).await.map_err(|e| e.to_string())?;
    let objects = s3client::list_objects_recursive(&client, &cfg, &path)
        .await
        .map_err(|e| e.to_string())?;

    let pins_root = state.cache_dir.lock().unwrap().join("pins");
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    let mut count = 0;
    for (key, size) in &objects {
        if is_junk_name(key.rsplit('/').next().unwrap_or(key)) {
            continue;
        }
        let pin = PinnedFile {
            id: Uuid::new_v4().to_string(),
            s3_key: key.clone(),
            bucket: cfg.bucket.clone(),
            local_path: pins_root.join(key).to_string_lossy().into_owned(),
            size: *size,
            last_synced: None,
            is_cached: false,
            etag: None,
        };
        db::upsert_pin(&conn, &pin).map_err(|e| e.to_string())?;
        count += 1;
    }

    // Start downloading the newly-pinned files.
    let db_path = state.db_path.clone();
    let max_bytes = *state.cache_max_mb.lock().unwrap() * 1024 * 1024;
    let progress = state.sync_progress.clone();
    let cfg2 = cfg.clone();
    tokio::spawn(async move {
        let _ = sync::sync_pins(db_path, cfg2, max_bytes, progress, app_handle).await;
    });

    Ok(count)
}

/// Unpin a whole folder: remove every pin under it and delete the cached files.
#[tauri::command]
pub async fn unpin_folder(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let cfg = state.s3_config.lock().unwrap().clone().ok_or("No S3 config saved")?;
    let prefix = s3client::build_prefix(&cfg, &path);
    let conn = db::open(&state.db_path).map_err(|e| e.to_string())?;
    let removed = db::delete_pins_under(&conn, &cfg.bucket, &prefix).map_err(|e| e.to_string())?;
    for p in removed {
        let _ = tokio::fs::remove_file(&p).await;
    }
    Ok(())
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

/// Force the mounted drive to re-list from S3 (rclone rc vfs/refresh) so files
/// added elsewhere — e.g. uploaded on the web — appear without a remount.
/// Best-effort: no-op when not mounted; rc errors are swallowed.
#[tauri::command]
pub async fn refresh_files(state: State<'_, AppState>) -> Result<(), String> {
    // Refresh every mounted filespace on its own rc port.
    let ports: Vec<u16> = {
        let mounts = state.mounts.lock().await;
        mounts.values().filter(|m| matches!(m.status, MountStatus::Mounted)).map(|m| m.rc_port).collect()
    };
    if ports.is_empty() {
        return Ok(());
    }
    let rclone_bin = mount::resolve_rclone_binary(&state.config_dir).map_err(|e| e.to_string())?;
    for port in ports {
        let _ = tokio::process::Command::new(&rclone_bin)
            .args(["rc", "--rc-addr", &format!("127.0.0.1:{}", port), "vfs/refresh", "recursive=true"])
            .output()
            .await;
    }
    Ok(())
}

/// Open the folder holding the rclone mount logs (rclone-<id>.log), so upload /
/// permission errors can be inspected when a copy isn't reaching the bucket.
#[tauri::command]
pub async fn reveal_logs(state: State<'_, AppState>) -> Result<(), String> {
    let dir = state.config_dir.clone();
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.to_string_lossy().into_owned();
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&p).spawn().map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer").arg(&p).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether macFUSE is installed — i.e. whether new mounts will be a true LOCAL
/// volume (vs an NFS network volume). The UI uses this to label the mount and to
/// prompt installing macFUSE for local-disk mode.
#[tauri::command]
pub fn macfuse_available() -> bool {
    mount::is_macfuse_available()
}

/// Open a URL in the default browser (e.g. the macFUSE download page).
#[tauri::command]
pub async fn open_url(url: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let r = std::process::Command::new("xdg-open").arg(&url).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

/// Open a native folder picker and return the chosen path (None if cancelled).
/// Used to choose the offline-cache location. Runs on a worker thread (async
/// command), so the blocking dialog call is safe.
#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle, default_path: Option<String>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let mut dlg = app.dialog().file().set_title("Choose cache folder");
    if let Some(p) = default_path {
        if !p.trim().is_empty() {
            dlg = dlg.set_directory(&p);
        }
    }
    let picked = dlg.blocking_pick_folder();
    Ok(picked.and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().into_owned()))
}

/// Live transfer activity on the mounted drive, read from rclone's rc API.
/// `active` = files currently moving (either direction), `uploading` = dirty
/// cache items being pushed to S3, `speed_bps` = aggregate bytes/sec. All zero
/// when nothing is transferring (or not mounted).
#[derive(serde::Serialize, Default, Clone)]
pub struct TransferStats {
    pub active: u32,
    pub uploading: u32,
    pub speed_bps: f64,
}

#[tauri::command]
pub async fn mount_transfer_stats(state: State<'_, AppState>) -> Result<TransferStats, String> {
    // Aggregate live activity across ALL mounted filespaces (each its own port).
    let ports: Vec<u16> = {
        let mounts = state.mounts.lock().await;
        mounts.values().filter(|m| matches!(m.status, MountStatus::Mounted)).map(|m| m.rc_port).collect()
    };
    if ports.is_empty() {
        return Ok(TransferStats::default());
    }
    let rclone_bin = mount::resolve_rclone_binary(&state.config_dir).map_err(|e| e.to_string())?;
    let mut stats = TransferStats::default();

    for port in ports {
        let addr = format!("127.0.0.1:{}", port);
        // core/stats → active transfers + aggregate speed (reads + writes).
        if let Ok(out) = tokio::process::Command::new(&rclone_bin)
            .args(["rc", "--rc-addr", &addr, "core/stats"])
            .output()
            .await
        {
            if out.status.success() {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    stats.speed_bps += v.get("speed").and_then(|x| x.as_f64()).unwrap_or(0.0);
                    stats.active += v.get("transferring").and_then(|x| x.as_array()).map(|a| a.len() as u32).unwrap_or(0);
                }
            }
        }
        // vfs/stats → uploads in progress/queued (distinguish upload vs download).
        if let Ok(out) = tokio::process::Command::new(&rclone_bin)
            .args(["rc", "--rc-addr", &addr, "vfs/stats"])
            .output()
            .await
        {
            if out.status.success() {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    if let Some(dc) = v.get("diskCache") {
                        let inprog = dc.get("uploadsInProgress").and_then(|x| x.as_u64()).unwrap_or(0);
                        let queued = dc.get("uploadsQueued").and_then(|x| x.as_u64()).unwrap_or(0);
                        stats.uploading += (inprog + queued) as u32;
                    }
                }
            }
        }
    }

    Ok(stats)
}

#[tauri::command]
pub async fn reveal_mount_point(state: State<'_, AppState>) -> Result<(), String> {
    // Reveal the SELECTED filespace's mount (the one being browsed).
    let id = state.active_filespace.lock().unwrap().as_ref().map(|a| a.id.clone());
    let mp = {
        let mounts = state.mounts.lock().await;
        id.as_ref()
            .and_then(|i| mounts.get(i))
            .and_then(|m| m.mount_point.clone())
            // Fall back to any mounted filespace if the selected one isn't mounted.
            .or_else(|| mounts.values().find_map(|m| m.mount_point.clone()))
            .ok_or("Not mounted")?
    };

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
