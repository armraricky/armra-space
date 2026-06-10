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

/// The branded home folder. macOS won't honor a custom icon on an NFS volume
/// root, so instead this is a real local folder we brand (green-Q) and mount
/// filespaces INSIDE — that's the entry point users see in Finder/the sidebar.
pub fn brand_base_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("ARMRA Space")
}

fn sanitize_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c == '/' || c == ':' { '-' } else { c })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "Filespace".to_string() } else { s }
}

/// Mount point for a filespace: <branded base>/<Filespace Name>.
pub fn mount_point_for(name: &str) -> PathBuf {
    brand_base_dir().join(sanitize_name(name))
}

/// Apply the bundled brand icon to a folder (macOS custom-icon bit via
/// NSWorkspace). Best-effort — never fails the mount.
#[cfg(target_os = "macos")]
pub fn set_folder_icon(folder: &std::path::Path, icns: &std::path::Path) {
    if !icns.exists() { return; }
    let script = format!(
        "use framework \"AppKit\"\n\
         set img to current application's NSImage's alloc()'s initWithContentsOfFile:\"{}\"\n\
         current application's NSWorkspace's sharedWorkspace()'s setIcon:img forFile:\"{}\" options:0",
        icns.to_string_lossy().replace('"', "\\\""),
        folder.to_string_lossy().replace('"', "\\\""),
    );
    let _ = std::process::Command::new("osascript")
        .args(["-l", "AppleScript", "-e", &script])
        .output();
}
#[cfg(not(target_os = "macos"))]
pub fn set_folder_icon(_folder: &std::path::Path, _icns: &std::path::Path) {}

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
    volname: &str,
) -> Result<Child> {
    // macOS: mount point must exist but be empty
    std::fs::create_dir_all(mount_point)?;
    std::fs::create_dir_all(cache_dir)?;

    let remote = format!("s3vault:{}", remote_path);

    // macOS: use rclone's NFS mount — it serves the remote over NFS on
    // localhost and mounts it with the OS's BUILT-IN NFS client. No macFUSE,
    // no kernel/system-extension approval, no reboot — the blocker that made
    // FUSE mounts a non-starter for normal users. Same VFS flags apply.
    // Windows keeps classic mount (WinFSP).
    #[cfg(target_os = "macos")]
    let mount_cmd = "nfsmount";
    #[cfg(not(target_os = "macos"))]
    let mount_cmd = "mount";

    let mut args: Vec<&str> = vec![
        mount_cmd,
        &remote,
        mount_point.to_str().unwrap(),
        "--config",
        config_path.to_str().unwrap(),
        "--vfs-cache-mode",
        "writes",           // write-through, reads go straight to S3
        "--cache-dir",      // global flag — VFS cache lands under <dir>/vfs ("--vfs-cache-dir" doesn't exist)
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
    // Name the mounted volume after the filespace (e.g. "Creative") instead of
    // the bucket / "s3vault". --volname is macOS/Windows only on nfsmount.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if !volname.is_empty() {
        args.push("--volname");
        args.push(volname);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = volname;

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
        // Plain `umount` is user-level for an NFS mount the user created — no
        // admin/password prompt. (diskutil unmount can escalate.) Killing the
        // rclone child above already stops its NFS server; this detaches the
        // mount point. `-f` only if a clean umount fails.
        let out = tokio::process::Command::new("umount")
            .arg(mp.to_str().unwrap())
            .output()
            .await;
        if out.map(|o| !o.status.success()).unwrap_or(true) {
            let _ = tokio::process::Command::new("umount")
                .args(["-f", mp.to_str().unwrap()])
                .output()
                .await;
        }
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

// Resolution order matters twice over on macOS:
//  - GUI apps launched from Finder get a minimal PATH (no /opt/homebrew/bin),
//    so `which` alone misses installed binaries ("os error 2").
//  - Homebrew's rclone is built WITHOUT FUSE-mount support (brew can't link
//    the macFUSE cask) and hard-fails `rclone mount`. So the app's own BUNDLED
//    official binary always wins, and /opt/homebrew is probed dead last.
#[cfg(target_os = "macos")]
const RCLONE_PATHS: &[&str] = &[
    "/usr/local/bin/rclone",    // official rclone.org installer location
    "/usr/bin/rclone",
    "/opt/homebrew/bin/rclone", // brew build — mount-disabled, last resort
];
#[cfg(target_os = "windows")]
const RCLONE_PATHS: &[&str] = &["C:\\Program Files\\rclone\\rclone.exe"];
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
const RCLONE_PATHS: &[&str] = &["/usr/local/bin/rclone", "/usr/bin/rclone"];

pub fn resolve_rclone_binary(app_dir: &PathBuf) -> Result<String> {
    // 1) The sidecar Tauri bundles next to the app executable (externalBin
    //    "binaries/rclone" → Contents/MacOS/rclone). Official build, always
    //    mount-capable — present in every packaged install.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sidecar = dir.join(if cfg!(windows) { "rclone.exe" } else { "rclone" });
            if sidecar.exists() {
                return Ok(sidecar.to_string_lossy().into_owned());
            }
        }
    }
    // 2) Dev runs: the staged sidecar in the repo (scripts/fetch-rclone.sh),
    //    via the legacy config-dir slot.
    let bundled = app_dir.join("binaries").join(format!(
        "rclone-{}",
        std::env::consts::ARCH
    ));
    if bundled.exists() {
        return Ok(bundled.to_string_lossy().into_owned());
    }
    // 3) System installs, then PATH.
    for p in RCLONE_PATHS {
        if std::path::Path::new(p).exists() {
            return Ok((*p).to_string());
        }
    }
    which::which("rclone")
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| anyhow::anyhow!("rclone isn’t available. Reinstall ARMRA Space (it ships with rclone built in), or install it from rclone.org/downloads."))
}
