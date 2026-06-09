import type { PinnedFile } from "../lib/tauri";
import { api } from "../lib/tauri";
import { formatBytes, formatDate, fileIcon } from "../lib/utils";

interface Props {
  pins: PinnedFile[];
  onPinsChange: () => void;
  onOpenFile: (path: string) => void;
}

export function PinsSidebar({ pins, onPinsChange, onOpenFile }: Props) {
  const cached = pins.filter((p) => p.is_cached);
  const pending = pins.filter((p) => !p.is_cached);

  const unpin = async (p: PinnedFile) => {
    await api.unpinFile(p.s3_key);
    onPinsChange();
  };

  const EntryRow = ({ p }: { p: PinnedFile }) => {
    const name = p.s3_key.split("/").pop() ?? p.s3_key;
    return (
      <div className={`pin-item ${p.is_cached ? "cached" : "pending"}`}>
        <span className="pin-icon">{fileIcon(name, false)}</span>
        <div className="pin-info">
          <div className="pin-name" title={p.s3_key}>{name}</div>
          <div className="pin-meta">
            {p.is_cached ? (
              <>
                <span className="badge-cached">✓ Cached</span>
                {" · "}
                {formatBytes(p.size)}
                {p.last_synced && " · " + formatDate(p.last_synced)}
              </>
            ) : (
              <>
                <span className="badge-pending">⏳ Pending</span>
                {" · "}{formatBytes(p.size)}
              </>
            )}
          </div>
        </div>
        <div className="pin-actions">
          {p.is_cached && (
            <button
              className="btn-ghost-sm"
              onClick={() => onOpenFile(p.local_path)}
              title="Open cached file"
            >
              ↗
            </button>
          )}
          <button
            className="btn-ghost-sm danger"
            onClick={() => unpin(p)}
            title="Remove pin"
          >
            ✕
          </button>
        </div>
      </div>
    );
  };

  return (
    <aside className="pins-sidebar">
      <div className="sidebar-header">
        <h3>Pinned Files</h3>
        <span className="badge">{pins.length}</span>
      </div>

      {pins.length === 0 && (
        <div className="sidebar-empty">
          No pins yet. Hit ⊕ on any file to cache it for offline access.
        </div>
      )}

      {cached.length > 0 && (
        <section>
          <div className="section-label">Cached ({cached.length})</div>
          {cached.map((p) => (
            <EntryRow key={p.id} p={p} />
          ))}
        </section>
      )}

      {pending.length > 0 && (
        <section>
          <div className="section-label">Pending ({pending.length})</div>
          {pending.map((p) => (
            <EntryRow key={p.id} p={p} />
          ))}
        </section>
      )}
    </aside>
  );
}
