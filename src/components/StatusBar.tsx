import type { MountStatus, SyncProgress } from "../lib/tauri";

interface Props {
  mountStatus: MountStatus;
  mountPoint?: string;
  syncProgress: SyncProgress | null;
  onMount: () => void;
  onUnmount: () => void;
  onReveal: () => void;
  onSync: () => void;
  hasConfig: boolean;
}

export function StatusBar({
  mountStatus,
  mountPoint,
  syncProgress,
  onMount,
  onUnmount,
  onReveal,
  onSync,
  hasConfig,
}: Props) {
  const isSyncing =
    syncProgress !== null &&
    syncProgress.total > 0 &&
    syncProgress.done < syncProgress.total;

  const dot =
    mountStatus === "mounted"
      ? "dot-green"
      : mountStatus === "mounting"
      ? "dot-yellow"
      : mountStatus === "error"
      ? "dot-red"
      : "dot-gray";

  return (
    <div className="status-bar">
      <div className="status-left">
        <span className={`dot ${dot}`} />
        <span className="mount-label">
          {mountStatus === "mounted"
            ? `Mounted at ${mountPoint ?? "…"}`
            : mountStatus === "mounting"
            ? "Mounting…"
            : mountStatus === "error"
            ? "Mount error"
            : "Not mounted"}
        </span>
      </div>

      <div className="status-right">
        {isSyncing && (
          <span className="sync-indicator">
            ↓ {syncProgress!.done}/{syncProgress!.total}
            {syncProgress!.current_key && (
              <span className="sync-file">
                {" "}
                {syncProgress!.current_key.split("/").pop()}
              </span>
            )}
          </span>
        )}

        {mountStatus === "mounted" && (
          <>
            <button className="btn-ghost" onClick={onReveal}>
              Open in Finder
            </button>
            <button className="btn-ghost" onClick={onSync} disabled={isSyncing}>
              {isSyncing ? "Syncing…" : "Sync Pins"}
            </button>
            <button className="btn-danger-sm" onClick={onUnmount}>
              Unmount
            </button>
          </>
        )}

        {mountStatus === "unmounted" && (
          <button className="btn-primary" onClick={onMount} disabled={!hasConfig}>
            Mount
          </button>
        )}

        {mountStatus === "mounting" && (
          <button className="btn-primary" disabled>
            Mounting…
          </button>
        )}

        {mountStatus === "error" && (
          <button className="btn-primary" onClick={onMount}>
            Retry Mount
          </button>
        )}
      </div>
    </div>
  );
}
