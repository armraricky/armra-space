import { useState, useEffect, useCallback } from "react";
import { api, onSyncProgress } from "./lib/tauri";
import type {
  MountInfo, Session, Filespace, ActiveFilespace, CacheConfig, Update, PinnedFile, SyncProgress, MountStats,
} from "./lib/tauri";
import { LoginScreen } from "./components/LoginScreen";
import { Settings } from "./components/Settings";
import { FilespaceDetail } from "./components/FilespaceDetail";
import { FileTree } from "./components/FileTree";
import { PinsSidebar } from "./components/PinsSidebar";
import "./App.css";

type View = "filespace" | "settings";
type DetailTab = "overview" | "files";

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [sessionChecked, setSessionChecked] = useState(false);
  const [view, setView] = useState<View>("filespace");
  const [tab, setTab] = useState<DetailTab>("overview");

  const [filespaces, setFilespaces] = useState<Filespace[]>([]);
  const [filespacesError, setFilespacesError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [selected, setSelected] = useState<Filespace | null>(null);
  const [active, setActive] = useState<ActiveFilespace | null>(null);
  const [opening, setOpening] = useState(false);

  const [mountError, setMountError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // All currently-mounted filespaces (multi-mount). Selecting a filespace loads
  // its creds for browsing without mounting; mounting ADDS it to this set.
  const [mounts, setMounts] = useState<MountInfo[]>([]);
  const mountOf = (id?: string | null) => mounts.find((m) => m.id === id && m.status === "mounted");
  const anyMounted = mounts.some((m) => m.status === "mounted");

  const [cacheConfig, setCacheConfig] = useState<CacheConfig | null>(null);
  const [pins, setPins] = useState<PinnedFile[]>([]);

  const [update, setUpdate] = useState<Update | null>(null);
  const [updating, setUpdating] = useState<string | null>(null);
  const [appVersion, setAppVersion] = useState<string>("");
  const [sync, setSync] = useState<SyncProgress | null>(null);
  const [xfer, setXfer] = useState<MountStats | null>(null);

  const loadFilespaces = useCallback(async () => {
    setRefreshing(true);
    try {
      const fs = await api.listFilespaces();
      setFilespaces(fs);
      setFilespacesError(null);
      return fs;
    } catch (e) {
      setFilespacesError(String(e));
      return [];
    } finally {
      setRefreshing(false);
    }
  }, []);

  const reloadMounts = useCallback(() => { api.getMounts().then(setMounts).catch(() => {}); }, []);

  const refreshStatus = useCallback(() => {
    api.getMounts().then(setMounts).catch(() => {});
    api.getCacheConfig().then(setCacheConfig).catch(() => {});
    api.listPins().then(setPins).catch(() => {});
  }, []);

  const refreshPins = useCallback(() => {
    api.listPins().then(setPins).catch(() => {});
    api.getCacheConfig().then(setCacheConfig).catch(() => {});
  }, []);

  useEffect(() => { api.getVersion().then(setAppVersion).catch(() => {}); }, []);

  // Live pin-download progress + reflect completions (pending → cached).
  useEffect(() => {
    const un = onSyncProgress((p) => {
      setSync(p);
      if (p.total > 0 && p.done >= p.total) setTimeout(() => { refreshPins(); setSync(null); }, 700);
    });
    return () => { un.then((f) => f()); };
  }, [refreshPins]);
  const syncing = !!sync && sync.total > 0 && sync.done < sync.total;

  useEffect(() => {
    api.currentSession()
      .then((s) => {
        setSession(s);
        if (s) { loadFilespaces(); refreshStatus(); api.getActiveFilespace().then(setActive); }
      })
      .catch(() => setSession(null))
      .finally(() => setSessionChecked(true));
    api.checkUpdate().then((u) => { if (u) setUpdate(u); }).catch(() => {});
  }, [loadFilespaces, refreshStatus]);

  // Re-check for updates periodically. Closing the window hides the app to the
  // tray (it keeps running), so the on-launch check alone means a long-running
  // instance never learns about new releases — this surfaces them without a
  // manual relaunch.
  useEffect(() => {
    const t = setInterval(() => {
      api.checkUpdate().then((u) => { if (u) setUpdate(u); }).catch(() => {});
    }, 30 * 60 * 1000); // every 30 minutes
    return () => clearInterval(t);
  }, []);

  // Keep the mounted drive current: S3 has no change-push, so poll a VFS refresh
  // while mounted — files added elsewhere (e.g. uploaded on the web) then show
  // up in Finder within ~15s instead of needing a reconnect.
  useEffect(() => {
    if (!anyMounted) return;
    const t = setInterval(() => { api.refreshFiles().catch(() => {}); }, 15000);
    return () => clearInterval(t);
  }, [anyMounted]);

  // Poll live transfer activity across all mounts (uploads/downloads to S3) so
  // we can show a moving indicator while bytes are in flight.
  useEffect(() => {
    if (!anyMounted) { setXfer(null); return; }
    let alive = true;
    const tick = async () => {
      try { const s = await api.mountTransferStats(); if (alive) setXfer(s); } catch { /* ignore */ }
    };
    tick();
    const t = setInterval(tick, 1500);
    return () => { alive = false; clearInterval(t); };
  }, [anyMounted]);

  // Auto-refresh STS creds before expiry for EVERY mounted filespace. A single
  // interval scans the mounted set and re-mints any within ~5 min of lapsing
  // (refresh_filespace remounts it with fresh creds, leaving others untouched).
  useEffect(() => {
    if (!anyMounted) return;
    const t = setInterval(async () => {
      const now = Date.now();
      let changed = false;
      for (const m of mounts) {
        if (m.status === "mounted" && m.expiration && m.expiration - now < 5 * 60 * 1000) {
          try { await api.refreshFilespace(m.id); changed = true; } catch { /* retry next tick */ }
        }
      }
      if (changed) reloadMounts();
    }, 60_000);
    return () => clearInterval(t);
  }, [anyMounted, mounts, reloadMounts]);

  const selectFilespace = async (fs: Filespace) => {
    setSelected(fs);
    setView("filespace");
    setTab("overview");
    setMountError(null);
    setOpening(true);
    try {
      const a = await api.openFilespace(fs.id); // mint scoped creds for this scope
      setActive(a);
      refreshStatus();
    } catch (e) {
      setMountError(String(e));
    } finally {
      setOpening(false);
    }
  };

  const handleAuthed = useCallback((s: Session) => { setSession(s); loadFilespaces(); refreshStatus(); }, [loadFilespaces, refreshStatus]);

  const openInBrowser = async () => {
    setBusy(true); setMountError(null);
    try {
      // Mount the selected filespace if it isn't already — ADDS it alongside any
      // others (no longer ejects the previously-mounted one).
      if (!mountOf(selected?.id)) {
        // Make sure the selected filespace's creds are loaded before mounting.
        if (active?.id !== selected?.id && selected) { const a = await api.openFilespace(selected.id); setActive(a); }
        const r = await api.mountBucket();
        await reloadMounts();
        if (r.status !== "mounted") { setMountError(r.error || "Mount failed"); return; }
      }
      await api.revealMountPoint();
    } catch (e) {
      setMountError(String(e)); reloadMounts();
    } finally { setBusy(false); }
  };

  const disconnectFs = async (id: string) => {
    setBusy(true);
    try { await api.unmountFilespace(id); }
    catch (e) { setMountError(String(e)); }
    finally { await reloadMounts(); setBusy(false); }
  };
  const disconnect = async () => { if (selected) await disconnectFs(selected.id); };

  const signOut = async () => {
    try { await Promise.all(mounts.map((m) => api.unmountFilespace(m.id))); } catch { /* ignore */ }
    await api.logout();
    setSession(null); setSelected(null); setActive(null); setFilespaces([]);
    setMounts([]); setView("filespace");
  };

  const installUpdate = async () => {
    if (!update) return;
    setUpdating("Downloading…");
    try { await api.installUpdate(update, (e) => { if (e.event === "Finished") setUpdating("Installing…"); }); }
    catch (e) { setUpdating(null); setMountError(`Update failed: ${String(e)}`); }
  };

  if (!sessionChecked) return <div className="app-loading">Loading…</div>;
  if (!session) return <LoginScreen onAuthed={handleAuthed} />;

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="sidebar-logo">
          <img src="/armra-icon.png" alt="ARMRA" className="armra-icon" />
          <span className="app-wordmark">ARMRA</span>
          <span className="app-product">Space</span>
        </div>

        <div className="sidebar-section-head">
          <span>Filespaces</span>
          <button className="icon-btn" title="Refresh" onClick={() => loadFilespaces()} disabled={refreshing}>
            <span className={refreshing ? "spin" : ""}>⟳</span>
          </button>
        </div>

        <nav className="fs-list">
          {filespacesError ? (
            <div className="fs-list-empty error-text">{filespacesError}</div>
          ) : filespaces.length === 0 ? (
            <div className="fs-list-empty">No filespaces yet — ask an admin to grant you access in ARMRA Quest.</div>
          ) : (
            filespaces.map((fs) => {
              const isMounted = !!mountOf(fs.id);
              const act = xfer?.per.find((p) => p.id === fs.id);
              const moving = !!act && (act.active > 0 || act.uploading > 0);
              const up = !!act && act.uploading > 0;
              return (
                <div
                  key={fs.id}
                  className={`fs-item ${selected?.id === fs.id ? "active" : ""}`}
                  onClick={() => selectFilespace(fs)}
                  title={`${fs.bucket}/${fs.prefix}`}
                  role="button"
                >
                  <span className="fs-item-glyph">◉</span>
                  <span className="fs-item-name">{fs.name}</span>
                  {moving && (
                    <span
                      title={`${up ? "Uploading" : "Downloading"}${act.speed_bps > 0 ? ` · ${fmtSpeed(act.speed_bps)}` : ""}`}
                      style={{ marginLeft: "auto", fontSize: 10.5, fontWeight: 700, color: "var(--accent)", whiteSpace: "nowrap" }}
                    >
                      {up ? "↑" : "↓"}{act.speed_bps > 0 ? ` ${fmtSpeed(act.speed_bps)}` : ""}
                    </span>
                  )}
                  {isMounted && (
                    <button
                      className="fs-eject"
                      title="Disconnect (unmount)"
                      onClick={(e) => { e.stopPropagation(); disconnectFs(fs.id); }}
                    >⏏</button>
                  )}
                  {isMounted && <span className="fs-item-dot on" title="mounted" />}
                </div>
              );
            })
          )}
        </nav>

        <div className="sidebar-foot">
          <button className={`sidebar-link ${view === "settings" ? "active" : ""}`} onClick={() => setView("settings")}>⚙ Settings</button>
          <div className="sidebar-account">
            <span className="sidebar-email" title={session.email}>{session.email}</span>
            <button className="sidebar-link sm" onClick={signOut}>Sign out</button>
          </div>
        </div>
      </aside>

      <main className="detail">
        {update && (
          <div className="update-banner">
            Version {update.version} is available.
            <button className="update-btn" disabled={!!updating} onClick={installUpdate}>{updating || "Update & restart"}</button>
          </div>
        )}

        {view === "settings" ? (
          <div className="detail-scroll"><Settings /></div>
        ) : selected ? (
          <>
            <div className="detail-tabs">
              <button className={`detail-tab ${tab === "overview" ? "active" : ""}`} onClick={() => setTab("overview")}>Overview</button>
              <button className={`detail-tab ${tab === "files" ? "active" : ""}`} onClick={() => setTab("files")}>Files &amp; Pins</button>
            </div>
            {tab === "overview" ? (
              <FilespaceDetail
                filespace={selected}
                active={active}
                mounted={!!mountOf(selected.id)}
                mountPoint={mountOf(selected.id)?.mount_point}
                cache={cacheConfig}
                pinsCount={pins.length}
                opening={opening}
                busy={busy}
                error={mountError}
                onOpen={openInBrowser}
                onDisconnect={disconnect}
                onManageCache={() => setView("settings")}
                onRevealCache={() => api.revealCacheDir()}
                onRefresh={() => api.refreshFiles().catch(() => {})}
              />
            ) : (
              <div className="browse-layout">
                <FileTree pins={pins} bucket={selected.name} onPinsChange={refreshPins} />
                <div className="pins-col">
                  <button className="btn-ghost pins-sync-btn" onClick={() => { api.startSync().catch(() => {}); }} disabled={syncing} title="Download all pending pins for offline use">
                    {syncing ? `⭳ Pinning ${sync!.done}/${sync!.total}…` : `⭳ Sync pins${pins.some((p) => !p.is_cached) ? ` (${pins.filter((p) => !p.is_cached).length} pending)` : ""}`}
                  </button>
                  {syncing && <PinProgress sync={sync!} />}
                  <PinsSidebar pins={pins} onPinsChange={refreshPins} onOpenFile={(p) => api.openInFinder(p)} />
                </div>
              </div>
            )}
          </>
        ) : (
          <div className="detail-empty">
            <img src="/armra-icon.png" alt="" className="detail-empty-icon" />
            <p>Select a filespace to connect it.</p>
          </div>
        )}

        {/* Global pin-progress bar — visible from any tab while pinning. */}
        {syncing && (
          <div className="pin-progress-global" title={sync!.current_key ? `Pinning ${sync!.current_key.split("/").pop()}` : "Pinning files"}>
            <div className="pin-progress-global-fill" style={{ width: `${Math.round((sync!.done / sync!.total) * 100)}%` }} />
          </div>
        )}

        {/* Live cloud transfer activity (uploads/downloads through the drive). */}
        <TransferIndicator xfer={xfer} />

        <div className="app-footer">
          {appVersion && <div className="app-version">v{appVersion}</div>}
          <div className="app-credit">Made by Ricky Mantilla</div>
        </div>
      </main>
    </div>
  );
}

function fmtSpeed(bps: number): string {
  if (bps >= 1_048_576) return `${(bps / 1_048_576).toFixed(1)} MB/s`;
  if (bps >= 1024) return `${(bps / 1024).toFixed(0)} KB/s`;
  return `${Math.round(bps)} B/s`;
}

function TransferIndicator({ xfer }: { xfer: MountStats | null }) {
  const t = xfer?.total;
  if (!t || (t.active === 0 && t.uploading === 0)) return null;
  const up = t.uploading > 0;
  return (
    <div className={`xfer-pill ${up ? "up" : "down"}`} title="Transferring files to/from the cloud">
      <span className="xfer-spinner" aria-hidden />
      <span className="xfer-arrow">{up ? "↑" : "↓"}</span>
      <span className="xfer-label">{up ? "Uploading" : "Downloading"}</span>
      {t.speed_bps > 0 && <span className="xfer-speed">{fmtSpeed(t.speed_bps)}</span>}
    </div>
  );
}

function PinProgress({ sync }: { sync: SyncProgress }) {
  const pct = sync.total > 0 ? Math.round((sync.done / sync.total) * 100) : 0;
  return (
    <div className="pin-progress">
      <div className="pin-progress-top"><span>Pinning {sync.done}/{sync.total}</span><span>{pct}%</span></div>
      <div className="pin-progress-bar"><div className="pin-progress-fill" style={{ width: `${pct}%` }} /></div>
      {sync.current_key && <div className="pin-progress-file" title={sync.current_key}>{sync.current_key.split("/").pop()}</div>}
    </div>
  );
}
