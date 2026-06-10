import { useState, useEffect, useCallback } from "react";
import { api } from "./lib/tauri";
import type {
  MountStatus, Session, Filespace, ActiveFilespace, CacheConfig, Update,
} from "./lib/tauri";
import { LoginScreen } from "./components/LoginScreen";
import { Settings } from "./components/Settings";
import { FilespaceDetail } from "./components/FilespaceDetail";
import "./App.css";

type View = "filespace" | "settings";

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [sessionChecked, setSessionChecked] = useState(false);
  const [view, setView] = useState<View>("filespace");

  const [filespaces, setFilespaces] = useState<Filespace[]>([]);
  const [filespacesError, setFilespacesError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [selected, setSelected] = useState<Filespace | null>(null);
  const [active, setActive] = useState<ActiveFilespace | null>(null);
  const [opening, setOpening] = useState(false);

  const [mountStatus, setMountStatus] = useState<MountStatus>("unmounted");
  const [mountPoint, setMountPoint] = useState<string | undefined>();
  const [mountError, setMountError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Which filespace is ACTUALLY mounted — distinct from the one selected in the
  // sidebar. Selecting a filespace loads its creds but doesn't mount it, so the
  // mounted indicator must track this, not the selection.
  const [mountedId, setMountedId] = useState<string | null>(null);

  const [cacheConfig, setCacheConfig] = useState<CacheConfig | null>(null);
  const [pinsCount, setPinsCount] = useState(0);

  const [update, setUpdate] = useState<Update | null>(null);
  const [updating, setUpdating] = useState<string | null>(null);

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

  const refreshStatus = useCallback(() => {
    api.getMountStatus().then((m) => { setMountStatus(m.status); setMountPoint(m.mount_point); });
    api.getCacheConfig().then(setCacheConfig).catch(() => {});
    api.listPins().then((p) => setPinsCount(p.length)).catch(() => {});
  }, []);

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

  // Auto-refresh STS creds ~5 min before expiry — only for the filespace that's
  // actually mounted (skip static / non-expiring, and skip when the selected
  // filespace isn't the mounted one).
  useEffect(() => {
    if (!active?.expiration || active.id !== mountedId) return;
    const delay = Math.max(30_000, active.expiration - Date.now() - 5 * 60 * 1000);
    const t = setTimeout(async () => {
      try {
        const a = await api.openFilespace(active.id);
        setActive(a);
        const r = await api.mountBucket();
        setMountStatus(r.status); setMountPoint(r.mount_point);
      } catch { /* surfaced on next action */ }
    }, delay);
    return () => clearTimeout(t);
  }, [active, mountedId]);

  const selectFilespace = async (fs: Filespace) => {
    setSelected(fs);
    setView("filespace");
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
      // Mount this filespace if it isn't the currently-mounted one.
      if (!(mountStatus === "mounted" && mountedId === selected?.id)) {
        const r = await api.mountBucket();
        setMountStatus(r.status); setMountPoint(r.mount_point);
        if (r.status !== "mounted") return;
        setMountedId(selected?.id ?? null);
      }
      await api.revealMountPoint();
    } catch (e) {
      setMountStatus("error"); setMountError(String(e));
    } finally { setBusy(false); }
  };

  const disconnect = async () => {
    setBusy(true);
    try { await api.unmountBucket(); setMountStatus("unmounted"); setMountPoint(undefined); setMountedId(null); }
    catch (e) { setMountError(String(e)); }
    finally { setBusy(false); }
  };

  const signOut = async () => {
    if (mountStatus === "mounted") { try { await api.unmountBucket(); } catch { /* ignore */ } }
    await api.logout();
    setSession(null); setSelected(null); setActive(null); setFilespaces([]);
    setMountStatus("unmounted"); setMountPoint(undefined); setMountedId(null); setView("filespace");
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
          <img src="/armra-icon.svg" alt="ARMRA" className="armra-icon" />
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
            filespaces.map((fs) => (
              <button
                key={fs.id}
                className={`fs-item ${selected?.id === fs.id ? "active" : ""}`}
                onClick={() => selectFilespace(fs)}
                title={`${fs.bucket}/${fs.prefix}`}
              >
                <span className="fs-item-glyph">◉</span>
                <span className="fs-item-name">{fs.name}</span>
                {mountedId === fs.id && mountStatus === "mounted" && <span className="fs-item-dot on" title="mounted" />}
              </button>
            ))
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
          <FilespaceDetail
            filespace={selected}
            active={active}
            mounted={mountStatus === "mounted" && mountedId === selected.id}
            mountPoint={mountPoint}
            cache={cacheConfig}
            pinsCount={pinsCount}
            opening={opening}
            busy={busy}
            error={mountError}
            onOpen={openInBrowser}
            onDisconnect={disconnect}
            onManageCache={() => setView("settings")}
            onRevealCache={() => api.revealCacheDir()}
          />
        ) : (
          <div className="detail-empty">
            <img src="/armra-icon.svg" alt="" className="detail-empty-icon" />
            <p>Select a filespace to connect it.</p>
          </div>
        )}

        <div className="app-credit">Made by Ricky Mantilla</div>
      </main>
    </div>
  );
}
