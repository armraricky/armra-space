import { useState, useEffect, useCallback } from "react";
import { api, onSyncProgress } from "./lib/tauri";
import type {
  PinnedFile, MountStatus, SyncProgress, Session, Filespace, ActiveFilespace, Update,
} from "./lib/tauri";
import { LoginScreen } from "./components/LoginScreen";
import { Settings } from "./components/Settings";
import { FileTree } from "./components/FileTree";
import { PinsSidebar } from "./components/PinsSidebar";
import { StatusBar } from "./components/StatusBar";
import "./App.css";

type View = "browser" | "settings";

export default function App() {
  const [view, setView] = useState<View>("browser");
  const [session, setSession] = useState<Session | null>(null);
  const [sessionChecked, setSessionChecked] = useState(false);

  const [filespaces, setFilespaces] = useState<Filespace[]>([]);
  const [filespacesError, setFilespacesError] = useState<string | null>(null);
  const [active, setActive] = useState<ActiveFilespace | null>(null);
  const [opening, setOpening] = useState<string | null>(null);

  const [mountStatus, setMountStatus] = useState<MountStatus>("unmounted");
  const [mountPoint, setMountPoint] = useState<string | undefined>();
  const [pins, setPins] = useState<PinnedFile[]>([]);
  const [syncProgress, setSyncProgress] = useState<SyncProgress | null>(null);
  const [mountError, setMountError] = useState<string | null>(null);

  const [update, setUpdate] = useState<Update | null>(null);
  const [updating, setUpdating] = useState<string | null>(null);

  const loadFilespaces = useCallback(() => {
    api.listFilespaces()
      .then((fs) => { setFilespaces(fs); setFilespacesError(null); })
      .catch((e) => setFilespacesError(String(e)));
  }, []);

  // Boot: validate the stored session, then load everything it gates.
  useEffect(() => {
    api.currentSession()
      .then((s) => {
        setSession(s);
        if (s) {
          loadFilespaces();
          api.getActiveFilespace().then(setActive);
          api.getMountStatus().then((m) => { setMountStatus(m.status); setMountPoint(m.mount_point); });
          api.listPins().then(setPins);
        }
      })
      .catch(() => setSession(null))
      .finally(() => setSessionChecked(true));

    // Silent update check on launch — never blocks startup.
    api.checkUpdate().then((u) => { if (u) setUpdate(u); }).catch(() => {});
  }, [loadFilespaces]);

  useEffect(() => {
    const unlisten = onSyncProgress((p) => {
      setSyncProgress(p);
      if (p.done === p.total && p.total > 0) {
        setTimeout(() => api.listPins().then(setPins), 500);
      }
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  const refreshPins = useCallback(() => { api.listPins().then(setPins); }, []);

  // STS credentials expire (≤1h). Re-mint 5 minutes before expiry and, if a
  // drive is mounted, remount with the fresh creds (mount tears down + respawns).
  // Static mode has expiration === null — nothing to refresh.
  useEffect(() => {
    if (!active?.expiration) return;
    const delay = Math.max(30_000, active.expiration - Date.now() - 5 * 60 * 1000);
    const t = setTimeout(async () => {
      try {
        const a = await api.openFilespace(active.id);
        setActive(a);
        if (mountStatus === "mounted") {
          const r = await api.mountBucket();
          setMountStatus(r.status);
          setMountPoint(r.mount_point);
        }
      } catch {
        // Refresh failed (offline / revoked) — the next user action surfaces it.
      }
    }, delay);
    return () => clearTimeout(t);
  }, [active, mountStatus]);

  const handleAuthed = useCallback((s: Session) => {
    setSession(s);
    loadFilespaces();
  }, [loadFilespaces]);

  const openFilespace = async (fs: Filespace) => {
    setOpening(fs.id);
    setMountError(null);
    try {
      const a = await api.openFilespace(fs.id);
      setActive(a);
      setView("browser");
    } catch (e) {
      setFilespacesError(String(e));
    } finally {
      setOpening(null);
    }
  };

  const handleMount = async () => {
    setMountStatus("mounting");
    setMountError(null);
    try {
      const result = await api.mountBucket();
      setMountStatus(result.status);
      setMountPoint(result.mount_point);
    } catch (e) {
      setMountStatus("error");
      setMountError(String(e));
    }
  };

  const handleUnmount = async () => {
    await api.unmountBucket();
    setMountStatus("unmounted");
    setMountPoint(undefined);
  };

  const handleSync = async () => {
    setSyncProgress({ total: 0, done: 0, errors: [] });
    await api.startSync();
  };

  const handleSignOut = async () => {
    await api.logout();
    if (mountStatus === "mounted") { try { await api.unmountBucket(); } catch { /* ignore */ } }
    setSession(null);
    setActive(null);
    setFilespaces([]);
    setPins([]);
    setMountStatus("unmounted");
    setMountPoint(undefined);
    setView("browser");
  };

  const installUpdate = async () => {
    if (!update) return;
    setUpdating("Downloading…");
    try {
      await api.installUpdate(update, (e) => {
        if (e.event === "Started") setUpdating("Downloading…");
        else if (e.event === "Finished") setUpdating("Installing…");
      });
    } catch (e) {
      setUpdating(null);
      setMountError(`Update failed: ${String(e)}`);
    }
  };

  if (!sessionChecked) return <div className="app-loading">Loading…</div>;
  if (!session) return <LoginScreen onAuthed={handleAuthed} />;

  return (
    <div className="app">
      <header className="app-header">
        <div className="header-left">
          <span className="armra-logo">
            <img src="/armra-icon.svg" alt="ARMRA" className="armra-icon" />
            <span className="app-wordmark">ARMRA</span>
            <span className="app-product">Space</span>
          </span>
          {active && (
            <>
              <span className="header-divider" />
              <span className="bucket-name">{active.name}</span>
              {active.role === "viewer" && <span className="fs-tag">read-only</span>}
              {active.mode === "static" && (
                <span className="fs-tag warn" title="This deployment's key can't mint scoped credentials — mounting with the shared key, locked to this folder.">
                  direct key
                </span>
              )}
            </>
          )}
        </div>
        <nav className="header-nav">
          <button className={`nav-btn ${view === "browser" ? "active" : ""}`} onClick={() => setView("browser")}>Files</button>
          <button className={`nav-btn ${view === "settings" ? "active" : ""}`} onClick={() => setView("settings")}>Settings</button>
          <span className="header-divider" />
          <span className="header-email" title={session.email}>{session.email}</span>
          <button className="nav-btn" onClick={handleSignOut}>Sign out</button>
        </nav>
      </header>

      {update && (
        <div className="update-banner">
          Version {update.version} is available.
          <button className="update-btn" disabled={!!updating} onClick={installUpdate}>
            {updating || "Update & restart"}
          </button>
        </div>
      )}

      {/* Filespace selector */}
      <div className="filespace-bar">
        {filespacesError ? (
          <span className="filespace-error">{filespacesError}</span>
        ) : filespaces.length === 0 ? (
          <span className="filespace-empty">No filespaces yet — ask an admin to grant you access in ARMRA Quest.</span>
        ) : (
          filespaces.map((fs) => (
            <button
              key={fs.id}
              className={`filespace-chip ${active?.id === fs.id ? "active" : ""}`}
              disabled={opening === fs.id}
              onClick={() => openFilespace(fs)}
              title={`${fs.bucket}/${fs.prefix} · ${fs.role}`}
            >
              {opening === fs.id ? "Opening…" : fs.name}
            </button>
          ))
        )}
      </div>

      {active && (
        <StatusBar
          mountStatus={mountStatus}
          mountPoint={mountPoint}
          syncProgress={syncProgress}
          onMount={handleMount}
          onUnmount={handleUnmount}
          onReveal={() => api.revealMountPoint()}
          onSync={handleSync}
          hasConfig={!!active}
        />
      )}

      {mountError && <div className="mount-error-banner">{mountError}</div>}

      <main className="app-main">
        {view === "settings" ? (
          <Settings />
        ) : (
          <div className="browser-layout">
            {active ? (
              <FileTree
                key={active.id}
                pins={pins}
                bucket={active.name}
                onPinsChange={refreshPins}
              />
            ) : (
              <div className="no-config">
                <p>Select a filespace above to browse and mount it.</p>
              </div>
            )}
            <PinsSidebar
              pins={pins}
              onPinsChange={refreshPins}
              onOpenFile={(path) => api.openInFinder(path)}
            />
          </div>
        )}
      </main>
    </div>
  );
}
