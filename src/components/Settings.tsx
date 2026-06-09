import { useState, useEffect, useRef } from "react";
import { api } from "../lib/tauri";
import type { CacheConfig, Update } from "../lib/tauri";

function formatMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${mb.toFixed(0)} MB`;
}

export function Settings() {
  const [version, setVersion] = useState<string | null>(null);
  const versionClicks = useRef(0);
  const [easterEgg, setEasterEgg] = useState(false);

  const [cacheConfig, setCacheConfig] = useState<CacheConfig | null>(null);
  const [cachePath, setCachePath] = useState("");
  const [cacheMaxMb, setCacheMaxMb] = useState(0);
  const [cacheSaving, setCacheSaving] = useState(false);
  const [cacheError, setCacheError] = useState<string | null>(null);
  const [cacheSaved, setCacheSaved] = useState(false);

  const [update, setUpdate] = useState<Update | null>(null);
  const [updateState, setUpdateState] = useState<string | null>(null);

  useEffect(() => {
    api.getVersion().then(setVersion).catch(() => {});
    api.getCacheConfig().then((c) => {
      setCacheConfig(c);
      setCachePath(c.path);
      setCacheMaxMb(c.max_mb);
    });
  }, []);

  const saveCache = async () => {
    if (!cachePath.trim()) { setCacheError("Path is required."); return; }
    setCacheSaving(true);
    setCacheError(null);
    setCacheSaved(false);
    try {
      await api.setCacheConfig(cachePath.trim(), cacheMaxMb);
      const updated = await api.getCacheConfig();
      setCacheConfig(updated);
      setCacheSaved(true);
      setTimeout(() => setCacheSaved(false), 2000);
    } catch (e) {
      setCacheError(String(e));
    } finally {
      setCacheSaving(false);
    }
  };

  const checkForUpdates = async () => {
    setUpdateState("Checking…");
    try {
      const u = await api.checkUpdate();
      if (u) { setUpdate(u); setUpdateState(`Version ${u.version} is available.`); }
      else { setUpdate(null); setUpdateState("You’re on the latest version."); }
    } catch (e) {
      setUpdateState(`Check failed: ${String(e)}`);
    }
  };

  const installUpdate = async () => {
    if (!update) return;
    setUpdateState("Downloading…");
    try {
      await api.installUpdate(update, (e) => {
        if (e.event === "Finished") setUpdateState("Installing…");
      });
    } catch (e) {
      setUpdateState(`Update failed: ${String(e)}`);
    }
  };

  const usedPct =
    cacheConfig && cacheMaxMb > 0
      ? Math.min(100, (cacheConfig.used_mb / cacheMaxMb) * 100)
      : 0;

  return (
    <div className="settings-scroll">
      {/* ── Cache ── */}
      <section className="settings-section">
        <h2>Offline Cache</h2>
        {cacheError && <div className="error-box">{cacheError}</div>}

        <label>
          Cache Location
          <div className="input-row">
            <input
              value={cachePath}
              onChange={(e) => setCachePath(e.target.value)}
              placeholder="/path/to/cache"
              className="input-grow"
            />
            <button className="btn-ghost" onClick={() => api.revealCacheDir()} title="Open in Finder">↗</button>
          </div>
        </label>

        <label>
          Cache Limit <span className="label-hint">0 = unlimited</span>
          <div className="cache-limit-row">
            <input
              type="number"
              min={0}
              step={256}
              value={cacheMaxMb}
              onChange={(e) => setCacheMaxMb(Math.max(0, Number(e.target.value)))}
              className="input-number"
            />
            <span className="input-unit">MB</span>
            {cacheMaxMb >= 1024 && <span className="input-unit-hint">({(cacheMaxMb / 1024).toFixed(1)} GB)</span>}
          </div>
        </label>

        {cacheConfig !== null && (
          <div className="cache-usage">
            <div className="cache-usage-row">
              <span className="cache-usage-label">Used</span>
              <span className="cache-usage-value">
                {formatMb(cacheConfig.used_mb)}
                {cacheMaxMb > 0 && ` of ${formatMb(cacheMaxMb)}`}
              </span>
            </div>
            {cacheMaxMb > 0 && (
              <div className="cache-bar-track">
                <div
                  className="cache-bar-fill"
                  style={{
                    width: `${usedPct}%`,
                    background: usedPct > 90 ? "var(--red)" : usedPct > 70 ? "var(--yellow)" : "var(--text-h)",
                  }}
                />
              </div>
            )}
          </div>
        )}

        <button onClick={saveCache} disabled={cacheSaving} className="btn-primary">
          {cacheSaving ? "Saving…" : cacheSaved ? "Saved" : "Save Cache Settings"}
        </button>
      </section>

      {/* ── Updates ── */}
      <section className="settings-section">
        <h2>Updates</h2>
        <p className="label-hint" style={{ marginBottom: 10 }}>
          ARMRA Space updates itself from GitHub Releases. {updateState || "Checks automatically on launch."}
        </p>
        <div className="input-row">
          <button className="btn-ghost" onClick={checkForUpdates}>Check for updates</button>
          {update && (
            <button className="btn-primary" onClick={installUpdate}>Update &amp; restart</button>
          )}
        </div>
      </section>

      {/* ── About ── */}
      <div className="settings-about">
        <span
          className="settings-version"
          onClick={() => {
            versionClicks.current += 1;
            if (versionClicks.current >= 7) { setEasterEgg(true); versionClicks.current = 0; }
          }}
        >
          ARMRA Space {version ? `v${version}` : "…"}
        </span>
        {easterEgg && (
          <span className="settings-egg" onClick={() => setEasterEgg(false)}>🐄 Revival of Health.</span>
        )}
      </div>
    </div>
  );
}
