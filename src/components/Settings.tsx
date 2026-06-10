import { useState, useEffect, useRef } from "react";
import { api } from "../lib/tauri";
import type { CacheConfig, Update } from "../lib/tauri";

function formatMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${mb.toFixed(0)} MB`;
}

// Quick-pick cache sizes (in MB). 0 = unlimited.
const CACHE_PRESETS = [
  { label: "5 GB", mb: 5120 },
  { label: "25 GB", mb: 25600 },
  { label: "50 GB", mb: 51200 },
  { label: "100 GB", mb: 102400 },
  { label: "Unlimited", mb: 0 },
];

export function Settings() {
  const [version, setVersion] = useState<string | null>(null);
  const versionClicks = useRef(0);
  const [easterEgg, setEasterEgg] = useState(false);

  const [cacheConfig, setCacheConfig] = useState<CacheConfig | null>(null);
  const [cachePath, setCachePath] = useState("");
  const [cacheMaxMb, setCacheMaxMb] = useState(0);
  const [customLimit, setCustomLimit] = useState(false);
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
      // If the saved limit isn't one of the presets, it's a custom value.
      setCustomLimit(!CACHE_PRESETS.some((p) => p.mb === c.max_mb));
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
            <button
              className="cache-folder-btn"
              onClick={async () => {
                const picked = await api.pickFolder(cachePath || undefined);
                if (picked) setCachePath(picked);
              }}
              title="Choose a folder"
            >
              <span className="cache-folder-icon">📁</span>
              <span className="cache-folder-path">{cachePath || "Choose a folder…"}</span>
            </button>
            <button className="btn-ghost" onClick={() => api.revealCacheDir()} title="Open in Finder">↗</button>
          </div>
        </label>

        <label>
          Cache Limit
          <div className="cache-preset-row">
            {CACHE_PRESETS.map((p) => (
              <button
                key={p.label}
                className={`cache-preset ${!customLimit && cacheMaxMb === p.mb ? "on" : ""}`}
                onClick={() => { setCustomLimit(false); setCacheMaxMb(p.mb); }}
              >
                {p.label}
              </button>
            ))}
            <button
              className={`cache-preset ${customLimit ? "on" : ""}`}
              onClick={() => setCustomLimit(true)}
            >
              Custom
            </button>
          </div>
          {customLimit && (
            <div className="cache-limit-row">
              <input
                type="number"
                min={1}
                step={1}
                value={cacheMaxMb > 0 ? +(cacheMaxMb / 1024).toFixed(1) : ""}
                onChange={(e) => setCacheMaxMb(Math.max(0, Math.round(Number(e.target.value) * 1024)))}
                className="input-number"
                placeholder="e.g. 30"
                autoFocus
              />
              <span className="input-unit">GB</span>
            </div>
          )}
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
