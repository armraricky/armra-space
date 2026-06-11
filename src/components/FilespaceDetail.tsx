import { useState, useEffect } from "react";
import { api } from "../lib/tauri";
import type { Filespace, ActiveFilespace, CacheConfig } from "../lib/tauri";

function fmtMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${mb.toFixed(0)} MB`;
}

const MODE_LABEL: Record<string, string> = {
  "assume-role": "Scoped (role)",
  federation: "Scoped (federated)",
  static: "Direct key",
};

interface Props {
  filespace: Filespace;
  active: ActiveFilespace | null;
  mounted: boolean; // is THIS filespace the one currently mounted
  mountPoint?: string;
  cache: CacheConfig | null;
  pinsCount: number;
  opening: boolean;
  busy: boolean;
  error: string | null;
  onOpen: () => void;
  onDisconnect: () => void;
  onManageCache: () => void;
  onRevealCache: () => void;
  onRefresh: () => void;
}

export function FilespaceDetail({
  filespace, active, mounted, mountPoint, cache, pinsCount,
  opening, busy, error, onOpen, onDisconnect, onManageCache, onRevealCache, onRefresh,
}: Props) {
  const [macfuse, setMacfuse] = useState<boolean | null>(null);
  useEffect(() => { api.macfuseAvailable().then(setMacfuse).catch(() => {}); }, []);

  const working = busy || opening;
  const status = working ? { label: "Connecting…", cls: "warn" }
    : mounted ? { label: "Connected", cls: "ok" }
    : { label: "Not connected", cls: "" };
  const isActive = active?.id === filespace.id;
  const usedMb = cache?.used_mb ?? 0;
  const limitMb = cache?.max_mb ?? 0;
  const usedPct = limitMb > 0 ? Math.min(100, (usedMb / limitMb) * 100) : 0;

  return (
    <div className="detail-scroll">
      {/* Header */}
      <div className="fs-header">
        <div className={`fs-drive ${mounted ? "on" : ""}`}>
          <img src="/armra-icon.png" alt="" />
        </div>
        <div className="fs-header-text">
          <h1>{filespace.name} {mounted ? <span className="fs-connected">is connected</span> : ""}</h1>
          <p>{mounted
            ? "To access the files in this filespace, open it in your file browser."
            : "Connect this filespace to mount it as a drive on this Mac."}</p>
        </div>
        <button className="btn-primary fs-open-btn" onClick={onOpen} disabled={busy || opening}>
          {busy ? "Working…" : mounted ? "Open filespace ↗" : "Connect & open"}
        </button>
      </div>

      {/* Mount mode — local disk (macFUSE) vs network drive (NFS). Matters for
          creative apps that won't work off a network volume. */}
      {macfuse === true && (
        <div className="fs-mode local" title="macFUSE is installed — this filespace mounts as a real local disk.">
          🖴 Mounts as a <strong>local drive</strong>
        </div>
      )}
      {macfuse === false && (
        <div className="fs-mode net">
          <span>🌐 Mounts as a <strong>network drive</strong>. For project work, install macFUSE to mount it as a local disk.</span>
          <button className="fs-mode-btn" onClick={() => api.openUrl("https://macfuse.github.io/")}>Install macFUSE</button>
        </div>
      )}

      {error && <div className="fs-error">{error}</div>}

      {/* Cards */}
      <div className="fs-cards">
        <section className="fs-card">
          <div className="fs-card-head"><span className="fs-card-ic storage">▤</span><h2>Storage</h2></div>
          <div className="fs-card-grid">
            <div><span className="fs-k">Cloud provider</span><span className="fs-v">AWS</span></div>
            <div><span className="fs-k">Region</span><span className="fs-v">{filespace.region || "—"}</span></div>
            <div><span className="fs-k">Bucket</span><span className="fs-v mono">{filespace.bucket}</span></div>
            <div><span className="fs-k">Folder (prefix)</span><span className="fs-v mono">{filespace.prefix}</span></div>
          </div>
        </section>

        <section className="fs-card">
          <div className="fs-card-head">
            <span className="fs-card-ic conn">↕</span><h2>Connection</h2>
            <span className={`fs-status ${status.cls}`}>● {status.label}</span>
          </div>
          <div className="fs-card-grid">
            <div><span className="fs-k">Access</span><span className="fs-v">{filespace.role}</span></div>
            <div><span className="fs-k">Credentials</span><span className="fs-v">{isActive && active ? (MODE_LABEL[active.mode] || active.mode) : "—"}</span></div>
            <div><span className="fs-k">Mount point</span><span className="fs-v mono">{mounted && mountPoint ? mountPoint.replace(/^.*\/(?=[^/]*\/[^/]*$)/, "~/") : "—"}</span></div>
            <div><span className="fs-k">Session</span><span className="fs-v">{isActive && active?.expiration ? "Auto-refreshing" : isActive && active?.mode === "static" ? "Long-lived" : "—"}</span></div>
          </div>
        </section>
      </div>

      {/* Pinned / cache */}
      <section className="fs-card fs-card-wide">
        <div className="fs-card-head"><span className="fs-card-ic pin">⚲</span><h2>Pinned Files &amp; Cache</h2></div>
        <div className="fs-cache-legend">
          <span><b className="dot used" /> Pinned files <em>{pinsCount}</em></span>
          <span><b className="dot lim" /> Cache used <em>{fmtMb(usedMb)}</em></span>
          <span><b className="dot cap" /> Cache limit <em>{limitMb > 0 ? fmtMb(limitMb) : "Unlimited"}</em></span>
        </div>
        {limitMb > 0 && (
          <div className="fs-cache-bar"><div className="fs-cache-fill" style={{ width: `${usedPct}%`, background: usedPct > 90 ? "var(--red)" : usedPct > 70 ? "var(--yellow)" : "var(--accent)" }} /></div>
        )}
        <div className="fs-card-links">
          <button className="link-btn" onClick={onManageCache}>Manage cache size</button>
          <button className="link-btn" onClick={onRevealCache}>Reveal cache folder ↗</button>
          {mounted && <button className="link-btn" onClick={onRefresh}>↻ Refresh files</button>}
        </div>
      </section>

      {mounted && (
        <button className="fs-disconnect" onClick={onDisconnect} disabled={busy}>⊘ Disconnect filespace</button>
      )}
    </div>
  );
}
