# ARMRA Space — Setup

ARMRA Space is the desktop client for **ARMRA Quest**. Users sign into Quest,
pick a **filespace** (a named bucket+prefix scope they've been granted), and mount
it as a Finder drive using short-lived, scoped AWS credentials. The app
auto-updates itself from GitHub Releases.

> **Architecture:** Quest = control plane (login, filespaces, access, STS minting).
> Space = thin client. S3 = data plane (the desktop talks straight to S3 with
> temporary credentials scoped to one bucket/prefix; Quest never sees file bytes).

---

## End-user prerequisites (macOS)

```bash
brew install rclone
brew install --cask macfuse      # required only for the Finder mount, not for browsing
```
Windows: [rclone](https://rclone.org/downloads/) + [WinFSP](https://winfsp.dev/rel/).

## Using the app
1. Launch → **Sign in with your browser** (or a pairing code from `armra.quest/space/pair`).
2. Your filespaces appear as chips. Click one to browse it.
3. **Mount** in the status bar to mount it as a Finder drive. Pin files for offline cache.

---

## Dev
```bash
npm install
npm run tauri:dev
```
Point the app at a non-prod Quest by setting the `quest_base` config key in its
SQLite DB (defaults to `https://armra.quest`).

## Build (distributable .app / .dmg)
```bash
npm run tauri:build              # local build — no signing key needed
```
Lands in `src-tauri/target/aarch64-apple-darwin/release/bundle/`.

---

## One-time backend & release setup

The pieces only you can do (AWS IAM, the GitHub release repo, signing keys, and
creating filespaces) are in **[INTEGRATION-SETUP.md](INTEGRATION-SETUP.md)**.

---

## How it works

| Layer | Detail |
|-------|--------|
| Auth | Browser PKCE deep-link (`armra-space://`) or pairing code → Quest issues a bearer token stored in the macOS Keychain. |
| Filespaces | `GET /api/space/filespaces` lists what you may mount; `POST /api/space/sts` mints scoped creds. |
| Mount | `rclone mount` → macFUSE/WinFSP at `~/ARMRA Space`, scoped to `bucket/prefix`. |
| File browser | Direct S3 API via `aws-sdk-s3` using the scoped temp creds — works without a mount. |
| Pinning / sync | Click ⊕; "Sync Pins" downloads to the configurable cache dir. |
| Updates | Checks GitHub Releases on launch + Settings → Updates. Minisign-verified. |
