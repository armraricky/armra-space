use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::process::{Child, Command};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MountStatus {
    Unmounted,
    Mounting,
    Mounted,
    Error,
}

pub struct MountState {
    pub status: MountStatus,
    pub mount_point: Option<PathBuf>,
    pub child: Option<Child>,
    pub error: Option<String>,
}

impl MountState {
    pub fn new() -> Self {
        Self {
            status: MountStatus::Unmounted,
            mount_point: None,
            child: None,
            error: None,
        }
    }
}

pub type SharedMountState = Arc<Mutex<MountState>>;

pub fn new_shared() -> SharedMountState {
    Arc::new(Mutex::new(MountState::new()))
}

/// rclone's S3 provider hint — AWS only when there's no custom endpoint.
/// Mirrors rcloneProvider() in Quest's lib/storage.js; rclone applies
/// AWS-specific behavior (list version, ETag/multipart) under provider=AWS,
/// which breaks B2 in particular.
fn rclone_provider(endpoint: Option<&str>) -> &'static str {
    match endpoint {
        None => "AWS",
        Some(ep) => {
            let e = ep.to_lowercase();
            if e.contains("r2.cloudflarestorage") { "Cloudflare" }
            else if e.contains("backblazeb2") { "Backblaze" }
            else if e.contains("digitaloceanspaces") { "DigitalOcean" }
            else if e.contains("wasabisys") { "Wasabi" }
            else { "Other" }
        }
    }
}

/// Write a minimal rclone config file and return its path.
///
/// `session_token` is set when mounting with short-lived STS credentials minted
/// for a filespace (the normal path); rclone needs it alongside the access key
/// and secret to authenticate temporary credentials. None for manual keys.
pub fn write_rclone_config(
    config_dir: &PathBuf,
    region: &str,
    access_key: &str,
    secret_key: &str,
    session_token: Option<&str>,
    endpoint: Option<&str>,
) -> Result<PathBuf> {
    std::fs::create_dir_all(config_dir)?;
    let config_path = config_dir.join("rclone.conf");

    let provider = rclone_provider(endpoint);
    let session_line = match session_token {
        Some(t) if !t.is_empty() => format!("session_token = {}\n", t),
        _ => String::new(),
    };
    let endpoint_line = if let Some(ep) = endpoint {
        format!("endpoint = {}\n", ep)
    } else {
        String::new()
    };

    let content = format!(
        "[s3vault]\n\
         type = s3\n\
         provider = {provider}\n\
         env_auth = false\n\
         access_key_id = {access_key}\n\
         secret_access_key = {secret_key}\n\
         {session_line}\
         region = {region}\n\
         {endpoint_line}\
         no_check_bucket = true\n"
    );

    std::fs::write(&config_path, content)?;
    Ok(config_path)
}

pub fn default_mount_point() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("ARMRA Space")
}

/// Spawn rclone mount. Returns the child process.
///
/// `remote_path` is the bucket, or "bucket/prefix" for a filespace scope, so the
/// mounted drive shows only that prefix (matching the STS-scoped credentials).
/// `read_only` mounts the drive read-only — used for 'viewer' grants, which
/// matters especially in static-credential mode where IAM can't enforce it.
pub async fn spawn_mount(
    rclone_bin: &str,
    config_path: &PathBuf,
    remote_path: &str,
    mount_point: &PathBuf,
    cache_dir: &PathBuf,
    read_only: bool,
) -> Result<Child> {
    // macOS: mount point must exist but be empty
    std::fs::create_dir_all(mount_point)?;
    std::fs::create_dir_all(cache_dir)?;

    let remote = format!("s3vault:{}", remote_path);

    let mut args: Vec<&str> = vec![
        "mount",
        &remote,
        mount_point.to_str().unwrap(),
        "--config",
        config_path.to_str().unwrap(),
        "--vfs-cache-mode",
        "writes",           // write-through, reads go straight to S3
        "--vfs-cache-dir",
        cache_dir.to_str().unwrap(),
        "--dir-cache-time",
        "30s",
        "--poll-interval",
        "15s",
        "--no-checksum",
        "--no-modtime",
        "--daemon=false",
        "--allow-non-empty",
        "--log-level",
        "ERROR",
    ];
    if read_only {
        args.push("--read-only");
    }

    let child = Command::new(rclone_bin)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    Ok(child)
}

/// Kill rclone and unmount (macOS: diskutil unmount).
pub async fn kill_mount(state: &mut MountState) -> Result<()> {
    if let Some(mut child) = state.child.take() {
        let _ = child.kill().await;
    }

    #[cfg(target_os = "macos")]
    if let Some(ref mp) = state.mount_point {
        let _ = tokio::process::Command::new("diskutil")
            .args(["unmount", "force", mp.to_str().unwrap()])
            .output()
            .await;
    }

    #[cfg(target_os = "windows")]
    if let Some(ref mp) = state.mount_point {
        let _ = tokio::process::Command::new("net")
            .args(["use", mp.to_str().unwrap(), "/delete"])
            .output()
            .await;
    }

    state.status = MountStatus::Unmounted;
    state.mount_point = None;
    Ok(())
}

pub fn resolve_rclone_binary(app_dir: &PathBuf) -> String {
    // Try bundled sidecar first, then system rclone.
    let bundled = app_dir.join("binaries").join(format!(
        "rclone-{}",
        std::env::consts::ARCH
    ));
    if bundled.exists() {
        return bundled.to_string_lossy().into_owned();
    }

    which::which("rclone")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "rclone".to_string())
}
