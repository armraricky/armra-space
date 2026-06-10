import { useState, useEffect, useCallback } from "react";
import { api } from "../lib/tauri";
import type { S3Entry, PinnedFile } from "../lib/tauri";
import { formatBytes, formatDate, fileIcon } from "../lib/utils";

interface Props {
  pins: PinnedFile[];
  bucket: string;
  onPinsChange: () => void;
}

interface BreadcrumbItem {
  name: string;
  path: string;
}

// OS-generated junk that should never show in the file list.
function isJunk(name: string): boolean {
  return (
    name === ".DS_Store" ||
    name === "Thumbs.db" ||
    name === ".localized" ||
    name.startsWith("._") ||
    name.startsWith(".Spotlight-V") ||
    name.startsWith(".Trash") ||
    name === ".fseventsd" ||
    name === ".TemporaryItems" ||
    name === ".apdisk"
  );
}

export function FileTree({ pins, bucket, onPinsChange }: Props) {
  const [path, setPath] = useState("");
  const [entries, setEntries] = useState<S3Entry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pinning, setPinning] = useState<Set<string>>(new Set());

  const pinnedKeys = new Set(pins.map((p) => p.s3_key));

  const load = useCallback(async (p: string) => {
    setPath(p);
    setError(null);
    // Paint the cached listing instantly (if any), then refresh live.
    let hadCache = false;
    try {
      const cached = await api.cachedListing(p);
      if (cached && cached.length) { setEntries(cached.filter((e) => !isJunk(e.name))); hadCache = true; }
    } catch { /* ignore */ }
    if (!hadCache) { setEntries([]); setLoading(true); }
    try {
      const result = await api.listFiles(p);
      setEntries(result.filter((e) => !isJunk(e.name)));
    } catch (e) {
      if (!hadCache) setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load("");
  }, [load]);

  const breadcrumbs = (): BreadcrumbItem[] => {
    if (!path) return [];
    const parts = path.split("/").filter(Boolean);
    return parts.map((name, i) => ({
      name,
      path: parts.slice(0, i + 1).join("/"),
    }));
  };

  const togglePin = async (entry: S3Entry) => {
    if (entry.is_dir) return;
    const key = entry.key;
    setPinning((s) => new Set(s).add(key));
    try {
      if (pinnedKeys.has(key)) {
        await api.unpinFile(key);
      } else {
        await api.pinFile(key, entry.size);
        // Kick off the download immediately so the pin doesn't sit "pending".
        api.startSync().catch(() => {});
      }
      onPinsChange();
    } finally {
      setPinning((s) => {
        const next = new Set(s);
        next.delete(key);
        return next;
      });
    }
  };

  return (
    <div className="file-tree">
      <div className="breadcrumbs">
        <span
          className="crumb clickable"
          onClick={() => load("")}
        >
          {bucket}
        </span>
        {breadcrumbs().map((b) => (
          <span key={b.path}>
            <span className="crumb-sep">/</span>
            <span className="crumb clickable" onClick={() => load(b.path)}>
              {b.name}
            </span>
          </span>
        ))}
      </div>

      {loading && <div className="loading">Loading…</div>}
      {error && <div className="error-box">{error}</div>}

      {!loading && !error && entries.length === 0 && (
        <div className="empty">Empty directory</div>
      )}

      {!loading && !error && entries.length > 0 && (
        <table className="file-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Size</th>
              <th>Modified</th>
              <th>Pin</th>
            </tr>
          </thead>
          <tbody>
            {entries.map((entry) => {
              const isPinned = pinnedKeys.has(entry.key);
              const isLoading = pinning.has(entry.key);
              return (
                <tr
                  key={entry.key}
                  className={`file-row ${isPinned ? "is-pinned" : ""}`}
                >
                  <td className="file-name">
                    <span className="file-icon">{fileIcon(entry.name, entry.is_dir)}</span>
                    {entry.is_dir ? (
                      <span
                        className="clickable dir-link"
                        onClick={() => load(entry.key.replace(/\/$/, ""))}
                      >
                        {entry.name}
                      </span>
                    ) : (
                      <span>{entry.name}</span>
                    )}
                  </td>
                  <td className="file-size">{formatBytes(entry.size)}</td>
                  <td className="file-date">{formatDate(entry.last_modified)}</td>
                  <td className="file-pin">
                    {!entry.is_dir && (
                      <button
                        className={`pin-btn ${isPinned ? "pinned" : ""}`}
                        onClick={() => togglePin(entry)}
                        disabled={isLoading}
                        title={isPinned ? "Unpin (remove offline cache)" : "Pin for offline access"}
                      >
                        {isLoading ? "…" : isPinned ? "⊘" : "⊕"}
                      </button>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}
