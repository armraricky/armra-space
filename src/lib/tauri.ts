import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion as _getVersion } from "@tauri-apps/api/app";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export interface S3Config {
  bucket: string;
  region: string;
  access_key: string;
  secret_key: string;
  endpoint?: string;
  prefix?: string;
  session_token?: string;
}

export interface S3Entry {
  key: string;
  name: string;
  is_dir: boolean;
  size: number;
  last_modified?: string;
  etag?: string;
}

export interface PinnedFile {
  id: string;
  s3_key: string;
  bucket: string;
  local_path: string;
  size: number;
  last_synced?: string;
  is_cached: boolean;
  etag?: string;
}

export type MountStatus = "unmounted" | "mounting" | "mounted" | "error";

export interface MountStatusResponse {
  status: MountStatus;
  mount_point?: string;
  error?: string;
}

export interface SyncProgress {
  total: number;
  done: number;
  current_key?: string;
  errors: string[];
}

export interface CacheConfig {
  path: string;
  max_mb: number; // 0 = unlimited
  used_mb: number;
}

export interface TransferStats {
  active: number;    // files currently transferring (either direction)
  uploading: number; // dirty cache items being pushed to S3
  speed_bps: number; // aggregate bytes/sec
}

// ── ARMRA Quest ──────────────────────────────────────────────────────────────
export interface Session {
  email: string;
  is_admin: boolean;
}

export interface Filespace {
  id: string;
  name: string;
  bucket: string;
  prefix: string;
  region?: string;
  role: string; // viewer | editor | owner
}

export interface ActiveFilespace {
  id: string;
  name: string;
  role: string;
  remote_path: string;
  expiration: number | null; // epoch ms; null = non-expiring (static mode)
  mode: string; // assume-role | federation | static
}

export type { Update };

export const api = {
  // Storage browsing + mount (uses the active filespace's scoped credentials)
  saveConfig: (config: S3Config) => invoke<void>("save_s3_config", { config }),
  loadConfig: () => invoke<S3Config | null>("load_s3_config"),
  mountBucket: () => invoke<MountStatusResponse>("mount_bucket"),
  unmountBucket: () => invoke<void>("unmount_bucket"),
  getMountStatus: () => invoke<MountStatusResponse>("get_mount_status"),
  listFiles: (path: string) => invoke<S3Entry[]>("list_files", { path }),
  cachedListing: (path: string) => invoke<S3Entry[] | null>("cached_listing", { path }),
  pinFile: (s3Key: string, size: number) =>
    invoke<PinnedFile>("pin_file", { s3Key, size }),
  unpinFile: (s3Key: string) => invoke<void>("unpin_file", { s3Key }),
  pinFolder: (path: string) => invoke<number>("pin_folder", { path }),
  unpinFolder: (path: string) => invoke<void>("unpin_folder", { path }),
  listPins: () => invoke<PinnedFile[]>("list_pins"),
  startSync: () => invoke<void>("start_sync"),
  getSyncProgress: () => invoke<SyncProgress>("get_sync_progress"),
  getCacheConfig: () => invoke<CacheConfig>("get_cache_config"),
  setCacheConfig: (path: string, maxMb: number) =>
    invoke<void>("set_cache_config", { path, maxMb }),
  revealCacheDir: () => invoke<void>("reveal_cache_dir"),
  pickFolder: (defaultPath?: string) => invoke<string | null>("pick_folder", { defaultPath: defaultPath ?? null }),
  openInFinder: (path: string) => invoke<void>("open_in_finder", { path }),
  revealMountPoint: () => invoke<void>("reveal_mount_point"),
  refreshFiles: () => invoke<void>("refresh_files"),
  mountTransferStats: () => invoke<TransferStats>("mount_transfer_stats"),
  macfuseAvailable: () => invoke<boolean>("macfuse_available"),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  getVersion: () => _getVersion(),

  // Auth (ARMRA Quest)
  beginLogin: async () => {
    const url = await invoke<string>("begin_login");
    await openUrl(url); // open the authorize page in the default browser
    return url;
  },
  currentSession: () => invoke<Session | null>("current_session"),
  submitPairingCode: (code: string) => invoke<Session>("submit_pairing_code", { code }),
  logout: () => invoke<void>("logout"),

  // Filespaces
  listFilespaces: () => invoke<Filespace[]>("list_filespaces"),
  openFilespace: (filespaceId: string) =>
    invoke<ActiveFilespace>("open_filespace", { filespaceId }),
  getActiveFilespace: () => invoke<ActiveFilespace | null>("get_active_filespace"),

  // Auto-update (GitHub Releases)
  checkUpdate: () => check(),
  installUpdate: async (update: Update, onEvent?: (e: { event: string; data?: unknown }) => void) => {
    await update.downloadAndInstall((p) => onEvent?.(p as { event: string; data?: unknown }));
    await relaunch();
  },
};

export function onSyncProgress(cb: (p: SyncProgress) => void) {
  return listen<SyncProgress>("sync-progress", (e) => cb(e.payload));
}

export function onAuthDone(cb: (s: Session) => void) {
  return listen<Session>("auth:done", (e) => cb(e.payload));
}

export function onAuthError(cb: (msg: string) => void) {
  return listen<string>("auth:error", (e) => cb(e.payload));
}
