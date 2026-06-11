import { useState, useEffect } from "react";
import { api, onAuthDone, onAuthError } from "../lib/tauri";
import type { Session } from "../lib/tauri";

export function LoginScreen({ onAuthed }: { onAuthed: (s: Session) => void }) {
  const [mode, setMode] = useState<"idle" | "waiting" | "pairing">("idle");
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // The browser PKCE flow completes via a deep-link → the Rust side emits
  // auth:done (or auth:error). Pairing completes inline via onAuthed.
  useEffect(() => {
    const undone = onAuthDone((s) => onAuthed(s));
    const unerr = onAuthError((msg) => {
      setError(msg);
      setMode("idle");
      setBusy(false);
    });
    return () => {
      undone.then((f) => f());
      unerr.then((f) => f());
    };
  }, [onAuthed]);

  async function browserLogin() {
    setError(null);
    setBusy(true);
    try {
      await api.beginLogin();
      setMode("waiting");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function pairingLogin() {
    if (!code.trim()) return;
    setError(null);
    setBusy(true);
    try {
      const s = await api.submitPairingCode(code.trim());
      onAuthed(s);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login-screen">
      <div className="login-card">
        <span className="login-logo">
          <img src="/armra-icon.png" alt="ARMRA" className="armra-icon" />
          <span className="app-wordmark">ARMRA</span>
          <span className="app-product">Space</span>
        </span>

        <h1 className="login-title">Sign in</h1>
        <p className="login-sub">
          Connect to ARMRA Quest to access your filespaces. You’ll mount them as drives on this Mac.
        </p>

        {mode === "waiting" ? (
          <div className="login-waiting">
            <div className="login-spinner" />
            <p>Finish signing in through your browser, then come back here.</p>
            <button className="btn-ghost" onClick={() => setMode("idle")}>Cancel</button>
          </div>
        ) : mode === "pairing" ? (
          <div className="login-pairing">
            <p className="login-hint">
              Open <strong>armra.quest/space/pair</strong> in a browser, then enter the code shown there.
            </p>
            <input
              className="login-input"
              value={code}
              onChange={(e) => setCode(e.target.value.toUpperCase().replace(/[^0-9A-Z]/g, ""))}
              placeholder="12-CHAR CODE"
              maxLength={12}
              onKeyDown={(e) => { if (e.key === "Enter") pairingLogin(); }}
            />
            <button className="btn-primary" disabled={busy || !code.trim()} onClick={pairingLogin}>
              {busy ? "Verifying…" : "Sign in with code"}
            </button>
            <button className="btn-ghost" onClick={() => { setMode("idle"); setError(null); }}>
              Back
            </button>
          </div>
        ) : (
          <div className="login-actions">
            <button className="btn-primary" disabled={busy} onClick={browserLogin}>
              {busy ? "Opening browser…" : "Sign in with your browser"}
            </button>
            <button className="btn-ghost" onClick={() => { setMode("pairing"); setError(null); }}>
              Enter a pairing code instead
            </button>
          </div>
        )}

        {error && <p className="login-error">{error}</p>}
      </div>
    </div>
  );
}
