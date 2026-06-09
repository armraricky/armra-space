use crate::commands::AppState;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};

const KEYRING_SERVICE: &str = "com.armra.space";
const KEYRING_USER: &str = "quest_token";

/// In-flight PKCE login state, held between begin_login and the deep-link
/// callback. verifier proves we initiated the flow; state guards against CSRF.
pub struct PendingPkce {
    pub verifier: String,
    pub state: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Session {
    pub email: String,
    pub is_admin: bool,
}

fn rand_b64url(n: usize) -> String {
    use rand::RngCore;
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE S256 challenge — base64url(sha256(verifier)), no padding. Must match
/// the server's pkceChallenge() in app/api/desktop/token/route.js.
fn pkce_challenge(verifier: &str) -> String {
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(h.finalize())
}

// ── OS keychain ──────────────────────────────────────────────────────────────
pub fn store_token(token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| e.to_string())?;
    entry.set_password(token).map_err(|e| e.to_string())
}
pub fn load_token() -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()?;
    entry.get_password().ok()
}
pub fn clear_token() {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        let _ = entry.delete_credential();
    }
}

// ── HTTP helpers ───────────────────────────────────────────────────────────
async fn exchange_code(base: &str, code: &str, verifier: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/desktop/token", base))
        .json(&serde_json::json!({ "grant_type": "authorization_code", "code": code, "code_verifier": verifier }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let ok = resp.status().is_success();
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !ok {
        return Err(body.get("error").and_then(|v| v.as_str()).unwrap_or("Token exchange failed").to_string());
    }
    body.get("token").and_then(|v| v.as_str()).map(|s| s.to_string()).ok_or_else(|| "No token in response".to_string())
}

pub async fn fetch_session(base: &str, token: &str) -> Result<Session, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/desktop/me", base))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(Session {
        email: body.get("email").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        is_admin: body.get("isAdmin").and_then(|v| v.as_bool()).unwrap_or(false),
    })
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// Start a browser PKCE login. Returns the authorize URL for the frontend to
/// open in the default browser; the flow completes via the deep-link callback.
#[tauri::command]
pub async fn begin_login(state: State<'_, AppState>) -> Result<String, String> {
    let verifier = rand_b64url(32);
    let challenge = pkce_challenge(&verifier);
    let st = rand_b64url(16);
    *state.pending_login.lock().unwrap() = Some(PendingPkce { verifier, state: st.clone() });
    let base = state.quest_base.clone();
    Ok(format!("{}/space/authorize?code_challenge={}&state={}&client=desktop", base, challenge, st))
}

/// Validate the stored token against Quest. Returns the session, or None if no
/// token / the token was revoked (401/403 → token cleared). Network errors keep
/// the token and surface as an Err so the UI can distinguish "offline" from
/// "signed out".
#[tauri::command]
pub async fn current_session(state: State<'_, AppState>) -> Result<Option<Session>, String> {
    let token = match load_token() {
        Some(t) => t,
        None => return Ok(None),
    };
    let base = state.quest_base.clone();
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/desktop/me", base))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if status.is_success() {
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        return Ok(Some(Session {
            email: body.get("email").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            is_admin: body.get("isAdmin").and_then(|v| v.as_bool()).unwrap_or(false),
        }));
    }
    if status.as_u16() == 401 || status.as_u16() == 403 {
        clear_token();
        return Ok(None);
    }
    Err(format!("Couldn’t reach ARMRA Quest (HTTP {})", status.as_u16()))
}

/// Pairing-code fallback: exchange a code the user got from armra.quest/space/pair.
#[tauri::command]
pub async fn submit_pairing_code(state: State<'_, AppState>, code: String) -> Result<Session, String> {
    let base = state.quest_base.clone();
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/desktop/token", base))
        .json(&serde_json::json!({ "grant_type": "pairing_code", "code": code.trim() }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let ok = resp.status().is_success();
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !ok {
        return Err(body.get("error").and_then(|v| v.as_str()).unwrap_or("Pairing failed").to_string());
    }
    let token = body.get("token").and_then(|v| v.as_str()).ok_or("No token in response")?.to_string();
    store_token(&token)?;
    fetch_session(&base, &token).await
}

/// Revoke the token server-side (best effort) and clear all local session state.
#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(token) = load_token() {
        let base = state.quest_base.clone();
        let client = reqwest::Client::new();
        let _ = client.delete(format!("{}/api/desktop/me", base)).bearer_auth(token).send().await;
    }
    clear_token();
    *state.s3_config.lock().unwrap() = None;
    *state.active_filespace.lock().unwrap() = None;
    Ok(())
}

/// Handle an armra-space:// deep-link callback. Parses code+state, verifies the
/// PKCE state, exchanges the code for a token, stores it, and emits "auth:done"
/// (or "auth:error") so the frontend can react.
pub fn handle_callback(app: &AppHandle, url: &str) {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return,
    };
    if parsed.scheme() != "armra-space" {
        return;
    }
    let mut code = None;
    let mut state_param = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state_param = Some(v.into_owned()),
            _ => {}
        }
    }
    let code = match code {
        Some(c) => c,
        None => return,
    };

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Extract everything that touches the (std) mutex BEFORE any await.
        let (base, pending) = {
            let st = app.state::<AppState>();
            let pending = st.pending_login.lock().unwrap().take();
            (st.quest_base.clone(), pending)
        };

        let result: Result<Session, String> = async {
            let pending = pending.ok_or("No login in progress")?;
            // Require state and require it to match — a callback without state is
            // rejected (defence-in-depth CSRF; legit callbacks always carry it).
            let sp = state_param.as_ref().ok_or("Missing state — login rejected.")?;
            if sp != &pending.state {
                return Err("State mismatch — login rejected.".to_string());
            }
            let token = exchange_code(&base, &code, &pending.verifier).await?;
            store_token(&token)?;
            fetch_session(&base, &token).await
        }
        .await;

        match result {
            Ok(sess) => {
                let _ = app.emit("auth:done", &sess);
            }
            Err(e) => {
                let _ = app.emit("auth:error", e);
            }
        }
    });
}
