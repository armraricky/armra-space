use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    pub rc_port: u16,             // this mount's private rclone rc port
    pub name: String,            // filespace name (for tray / UI)
    pub expiration: Option<i64>, // STS expiry (epoch ms), for refresh scheduling
}

impl MountState {
    pub fn new() -> Self {
        Self {
            status: MountStatus::Unmounted,
            mount_point: None,
            child: None,
            error: None,
            rc_port: 0,
            name: String::new(),
            expiration: None,
        }
    }
}

// All currently-mounted filespaces, keyed by filespace id. Multiple filespaces
// can be mounted at once — each is its own rclone process with a private rc port
// and its own config file.
pub type Mounts = Arc<Mutex<HashMap<String, MountState>>>;

pub fn new_mounts() -> Mounts {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Pick an rc port not already used by a live mount (rclone needs a unique
/// 127.0.0.1:<port> per process for the remote-control API).
pub fn pick_rc_port(used: &[u16]) -> u16 {
    let mut port = 5572u16;
    while used.contains(&port) {
        port += 1;
    }
    port
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
    file_stem: &str,
    region: &str,
    access_key: &str,
    secret_key: &str,
    session_token: Option<&str>,
    endpoint: Option<&str>,
    accelerate: bool,
) -> Result<PathBuf> {
    std::fs::create_dir_all(config_dir)?;
    // Per-filespace config file so multiple mounts can hold DIFFERENT creds at
    // once (a single shared rclone.conf would have them clobber each other).
    let safe: String = file_stem.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    let config_path = config_dir.join(format!("rclone-{}.conf", safe));

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
    // S3 Transfer Acceleration (AWS only) — rclone uses the s3-accelerate edge endpoint.
    let accel_line = if accelerate && endpoint.is_none() { "use_accelerate_endpoint = true\n" } else { "" };

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
         {accel_line}\
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
    cache_max_mb: u64,
    rc_port: u16,
) -> Result<Child> {
    // macOS: mount point must exist but be empty
    std::fs::create_dir_all(mount_point)?;
    std::fs::create_dir_all(cache_dir)?;

    // Stop macOS from writing .DS_Store onto network volumes (belt-and-suspenders
    // with the rclone --exclude filters below). Best-effort; takes effect for new
    // mounts. The user may need to relaunch Finder for it to fully apply.
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("defaults")
            .args(["write", "com.apple.desktopservices", "DSDontWriteNetworkStores", "-bool", "true"])
            .status();
    }

    let remote = format!("s3vault:{}", remote_path);
    // Cap the read cache to the user's configured limit (0 = unlimited → off).
    let cache_max = if cache_max_mb > 0 { format!("{}M", cache_max_mb) } else { "off".to_string() };
    // Private rc endpoint for THIS mount (each mounted filespace gets its own).
    let rc_addr = format!("127.0.0.1:{}", rc_port);

    // macOS mount strategy:
    //  - macFUSE installed → classic FUSE `mount` with `-o local`, which gives a
    //    real LOCAL volume. Critical for creative apps that refuse network drives
    //    for scratch/cache/lock files when working on project files.
    //  - otherwise → NFS mount (built-in client, zero install friction, but the
    //    OS classifies it as a network volume).
    // Windows keeps classic mount (WinFSP). Same VFS flags apply to all.
    #[cfg(target_os = "macos")]
    let use_macfuse = is_macfuse_available();
    #[cfg(target_os = "macos")]
    let mount_cmd = if use_macfuse { "mount" } else { "nfsmount" };
    #[cfg(not(target_os = "macos"))]
    let mount_cmd = "mount";

    // macFUSE volume icon: drop the bundled ARMRA icon next to the config and
    // point macFUSE's `volicon=` option at it so the mounted drive shows the
    // brand icon in Finder instead of the generic disk. Best-effort; macFUSE-only.
    #[cfg(target_os = "macos")]
    let volicon_opt: Option<String> = if use_macfuse {
        config_path.parent().and_then(|dir| {
            let path = dir.join("ARMRA Space.icns");
            std::fs::write(&path, include_bytes!("../icons/icon.icns")).ok()?;
            Some(format!("volicon={}", path.to_string_lossy()))
        })
    } else {
        None
    };

    let mut args: Vec<&str> = vec![
        mount_cmd,
        &remote,
        mount_point.to_str().unwrap(),
        "--config",
        config_path.to_str().unwrap(),
        // Full VFS cache: reads land on local disk, so re-opens are instant and
        // streaming is smooth (writes alone left every read going to S3).
        "--vfs-cache-mode", "full",
        "--cache-dir", cache_dir.to_str().unwrap(),
        "--vfs-cache-max-size", &cache_max,
        "--vfs-cache-max-age", "168h",
        // Prefetch ahead of the read head + pull large files as parallel chunks
        // — the difference between choppy and smooth video/large-asset playback.
        "--vfs-read-ahead", "256M",
        "--buffer-size", "64M",
        "--vfs-read-chunk-size", "16M",
        "--vfs-read-chunk-size-limit", "256M",
        "--vfs-read-chunk-streams", "4",
        "--transfers", "8",
        // Short dir cache so files added elsewhere (e.g. uploaded on the web)
        // appear quickly on re-listing. S3 has no change-push, so this + the
        // on-demand vfs/refresh below (rc) is how the mount stays current.
        "--dir-cache-time", "10s",
        "--poll-interval", "10s",
        "--no-checksum",
        // Use S3's LastModified as the file's mod time (the real upload date) so
        // Finder shows correct dates. NOT --no-modtime (that shows a placeholder
        // ~1999 date); --use-server-modtime avoids the per-object metadata HEAD
        // that plain modtime reads would cost, so it stays fast.
        "--use-server-modtime",
        // Treat `<folder>/` marker objects as real directories. They carry a
        // LastModified (set when the folder was created), so with
        // --use-server-modtime a folder shows its real creation date instead of
        // rclone's placeholder (~1999). rclone also writes a marker on mkdir, so
        // folders made in Finder get a date too.
        "--s3-directory-markers",
        // Remote-control endpoint so the app can force an immediate listing
        // refresh (vfs/refresh) after/while files change — no remount needed.
        "--rc",
        "--rc-addr", &rc_addr,
        "--rc-no-auth",
        // NOTE: do NOT use --exclude here. On a writable mount, when Finder copies
        // a file it also writes the AppleDouble sidecar (._name) and .DS_Store;
        // if rclone is told to exclude those, the create fails and Finder aborts
        // the entire copy with "error code -8062". We keep junk OUT another way:
        //   • .DS_Store on network (NFS) mounts → DSDontWriteNetworkStores (above)
        //   • AppleDouble on macFUSE mounts → the `noappledouble` option (below)
        //   • anything that still lands → hidden in-app by is_junk_name() on list
        "--daemon=false",
        "--allow-non-empty",
        "--log-level", "ERROR",
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

    // macFUSE-only mount options: mark the volume as LOCAL (so Finder/Spotlight
    // and apps treat it like an internal disk) and suppress AppleDouble junk.
    #[cfg(target_os = "macos")]
    if use_macfuse {
        args.push("--option");
        args.push("local");
        args.push("--option");
        args.push("noappledouble");
        if let Some(ref vi) = volicon_opt {
            args.push("--option");
            args.push(vi);
        }
    }

    let child = Command::new(rclone_bin)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    Ok(child)
}

/// True if macFUSE (or legacy osxfuse) is installed, so we can mount a real
/// LOCAL volume instead of an NFS network volume.
#[cfg(target_os = "macos")]
pub fn is_macfuse_available() -> bool {
    std::path::Path::new("/Library/Filesystems/macfuse.fs").exists()
        || std::path::Path::new("/Library/Filesystems/osxfuse.fs").exists()
}
#[cfg(not(target_os = "macos"))]
pub fn is_macfuse_available() -> bool {
    cfg!(windows) // WinFSP path is always a local-style mount
}

/// Is something currently mounted at this path? (Detects a stale mount left by
/// a previous app run — the new process has no child handle for it.)
pub fn is_path_mounted(mount_point: &PathBuf) -> bool {
    let Some(mp) = mount_point.to_str() else { return false };
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("/sbin/mount").output() {
            let listing = String::from_utf8_lossy(&out.stdout);
            // mount lines look like: "…  on /Users/me/ARMRA Space/Creative (nfs, …)"
            return listing.lines().any(|l| l.contains(&format!(" on {} ", mp)));
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = mp;
        false
    }
}

/// Force-detach a stale OS-level mount at this path (left over from a prior app
/// run / crash) and kill the orphaned rclone serving it, so a fresh mount can
/// take the path cleanly instead of rclone erroring "already mounted".
pub async fn force_unmount_stale(mount_point: &PathBuf) {
    let Some(mp) = mount_point.to_str() else { return };
    #[cfg(target_os = "macos")]
    {
        // Detach the mount (try clean, then forced).
        let out = tokio::process::Command::new("umount").arg(mp).output().await;
        if out.map(|o| !o.status.success()).unwrap_or(true) {
            let _ = tokio::process::Command::new("umount").args(["-f", mp]).output().await;
        }
        // Kill any orphaned rclone still serving this exact mount point. Scoped
        // to the unique path so other filespaces' mounts are untouched.
        let _ = tokio::process::Command::new("pkill").args(["-f", mp]).output().await;
    }
    #[cfg(target_os = "windows")]
    {
        let _ = tokio::process::Command::new("net").args(["use", mp, "/delete"]).output().await;
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let _ = mp;
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
