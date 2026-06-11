import { useState, useEffect } from "react";
import { api } from "../lib/tauri";
import type { Filespace, ActiveFilespace, CacheConfig, StorageInfo } from "../lib/tauri";

function fmtMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${mb.toFixed(0)} MB`;
}

function fmtBytes(b: number): string {
  if (b >= 1e9) return `${(b / 1e9).toFixed(2)} GB`;
  if (b >= 1e6) return `${(b / 1e6).toFixed(1)} MB`;
  if (b >= 1e3) return `${(b / 1e3).toFixed(0)} KB`;
  return `${b} B`;
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
  section?: "overview" | "diagnostics"; // which tab's content to render
  onOpen: () => void;
  onDisconnect: () => void;
  onManageCache: () => void;
  onRevealCache: () => void;
  onRefresh: () => void;
}

export function FilespaceDetail({
  filespace, active, mounted, mountPoint, cache, pinsCount,
  opening, busy, error, section = "overview",
  onOpen, onDisconnect, onManageCache, onRevealCache, onRefresh,
}: Props) {
  const [macfuse, setMacfuse] = useState<boolean | null>(null);
  useEffect(() => { api.macfuseAvailable().then(setMacfuse).catch(() => {}); }, []);

  // Total bytes stored in this filespace (via rclone size on its mount). Slow on
  // big buckets, so fetch once when connected (and only when its card is shown).
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [sizing, setSizing] = useState(false);
  async function loadStorage() {
    setSizing(true);
    try { setStorage(await api.filespaceStorageUsed(filespace.id)); }
    catch { setStorage(null); }
    finally { setSizing(false); }
  }
  useEffect(() => {
    setStorage(null);
    if (mounted && section === "overview") loadStorage();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mounted, filespace.id, section]);

  const working = busy || opening;
  const status = working ? { label: "Connecting…", cls: "warn" }
    : mounted ? { label: "Connected", cls: "ok" }
    : { label: "Not connected", cls: "" };
  const isActive = active?.id === filespace.id;
  const usedMb = cache?.used_mb ?? 0;
  const limitMb = cache?.max_mb ?? 0;
  const usedPct = limitMb > 0 ? Math.min(100, (usedMb / limitMb) * 100) : 0;

  // ── Diagnostics tab: the technical detail, kept out of the clean overview ──
  if (section === "diagnostics") {
    return (
      <div className="detail-scroll">
        <div className="fs-diag-head">
          <h2>{filespace.name} · Diagnostics</h2>
          <p>Connection details and troubleshooting tools. Nothing here is needed for everyday use.</p>
        </div>

        <section className="fs-card fs-card-wide">
          <div className="fs-card-head">
            <span className="fs-card-ic conn">↕</span><h2>Connection</h2>
            <span className={`fs-status ${status.cls}`}>● {status.label}</span>
          </div>
          <div className="fs-card-grid">
            <div><span className="fs-k">Access</span><span className="fs-v">{filespace.role}</span></div>
            <div><span className="fs-k">Credentials</span><span className="fs-v">{isActive && active ? (MODE_LABEL[active.mode] || active.mode) : "—"}</span></div>
            <div><span className="fs-k">Drive type</span><span className="fs-v">{macfuse === true ? "Local disk (macFUSE)" : macfuse === false ? "Network drive" : "—"}</span></div>
            <div><span className="fs-k">Region</span><span className="fs-v">{filespace.region || "—"}</span></div>
            <div><span className="fs-k">Mount point</span><span className="fs-v mono">{mounted && mountPoint ? mountPoint.replace(/^.*\/(?=[^/]*\/[^/]*$)/, "~/") : "—"}</span></div>
            <div><span className="fs-k">Session</span><span className="fs-v">{isActive && active?.expiration ? "Auto-refreshing" : isActive && active?.mode === "static" ? "Long-lived" : "—"}</span></div>
          </div>
        </section>

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
            <button className="link-btn" onClick={() => api.revealLogs().catch(() => {})}>Reveal logs ↗</button>
            {mounted && <button className="link-btn" onClick={onRefresh}>↻ Refresh files</button>}
          </div>
        </section>
      </div>
    );
  }

  // ── Overview tab: lean essentials, sized to fit without scrolling ──
  return (
    <div className="detail-scroll">
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

      <section className="fs-card fs-card-wide">
        <div className="fs-card-head">
          <span className="fs-card-ic storage">▤</span><h2>Storage</h2>
          <span className={`fs-status ${status.cls}`}>● {status.label}</span>
        </div>
        <div className="fs-card-grid">
          <div><span className="fs-k">Cloud provider</span><span className="fs-v">AWS</span></div>
          <div><span className="fs-k">Region</span><span className="fs-v">{filespace.region || "—"}</span></div>
          <div><span className="fs-k">Bucket</span><span className="fs-v mono">{filespace.bucket}</span></div>
          <div><span className="fs-k">Folder (prefix)</span><span className="fs-v mono">{filespace.prefix}</span></div>
          <div>
            <span className="fs-k">Total stored</span>
            <span className="fs-v" style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
              {!mounted ? "Connect to view"
                : sizing ? "Calculating…"
                : storage ? `${fmtBytes(storage.bytes)} · ${storage.count.toLocaleString()} files`
                : "—"}
              {mounted && !sizing && (
                <button onClick={loadStorage} title="Recalculate" style={{ background: "none", border: "none", cursor: "pointer", color: "var(--accent)", fontSize: 13, padding: 0, lineHeight: 1 }}>↻</button>
              )}
            </span>
          </div>
        </div>
      </section>

      {mounted && (
        <button className="fs-disconnect" onClick={onDisconnect} disabled={busy}>⊘ Disconnect filespace</button>
      )}
    </div>
  );
}
