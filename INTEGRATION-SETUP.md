# ARMRA Space — Backend & Release Setup

The one-time setup only *you* can do: AWS IAM, the GitHub release repo + signing
keys, deploying the Quest backend, and creating filespaces.

---

## 1. AWS IAM (enables scoped mounts)

When a user opens a filespace, Quest calls **STS AssumeRole** to mint short-lived
credentials scoped to exactly that bucket+prefix. Set this up once.

### a. Role `quest-filespace-access`
Trust policy (gated by ExternalId to prevent the confused-deputy problem):
```json
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Principal": { "AWS": "arn:aws:iam::<ACCOUNT_ID>:user/<YOUR_MINTER_USER>" },
    "Action": "sts:AssumeRole",
    "Condition": { "StringEquals": { "sts:ExternalId": "armra-quest" } }
  }]
}
```
Permissions policy on the role (broad — the per-request session policy Quest
attaches is the real narrowing intersection):
```json
{
  "Version": "2012-10-17",
  "Statement": [
    { "Effect": "Allow", "Action": ["s3:ListBucket","s3:GetBucketLocation"], "Resource": "arn:aws:s3:::<YOUR_BUCKET>" },
    { "Effect": "Allow", "Action": ["s3:GetObject","s3:PutObject","s3:DeleteObject"], "Resource": "arn:aws:s3:::<YOUR_BUCKET>/*" }
  ]
}
```

### b. Minter user
The IAM user whose keys you already store in **Quest → Admin → Storage** needs
only one extra permission:
```json
{ "Version": "2012-10-17",
  "Statement": [{ "Effect": "Allow", "Action": "sts:AssumeRole",
    "Resource": "arn:aws:iam::<ACCOUNT_ID>:role/quest-filespace-access" }] }
```

### c. Tell Quest the role ARN
**Quest → Admin → Storage** → set `roleArn` (stored at `storage.config.roleArn`) to
`arn:aws:iam::<ACCOUNT_ID>:role/quest-filespace-access`.

> ExternalId is hard-coded as `armra-quest` (`lib/storage.js → STS_EXTERNAL_ID`).
> Scoped mounts require **AWS S3** — R2/MinIO/Spaces/B2 aren't supported for STS.

---

## 2. Deploy the Quest backend

Changes live on **`feat/filespaces`** of `armraricky/armra-dashboard`
(durable clone at `~/Documents/armra-dashboard`). All new DB tables are created
lazily — additive, no migration, safe to roll back. `next build` passes.

```bash
cd ~/Documents/armra-dashboard
git add -A && git commit -m "feat: filespaces + desktop auth + STS minting"
git push -u origin feat/filespaces
# open a PR, review, merge to main → Vercel deploys to armra.quest
```
Vercel installs the new `@aws-sdk/client-sts` dep automatically.

**What was added to Quest:**
- `lib/db.js` — `filespaces`, `filespace_access`, `desktop_auth_codes`, `desktop_tokens` tables + helpers
- `lib/storage.js` — `roleArn`, `s3AssumeRoleForFilespace`, `buildFilespaceSessionPolicy`
- `lib/desktop-guard.js` — bearer-token guard with live allowlist re-check
- `app/api/desktop/{authorize,token,me}` — PKCE + pairing device auth
- `app/api/space/{filespaces,sts}` — list + mint (bearer-gated)
- `app/api/admin/filespaces` + `/[id]/access` — admin CRUD
- `app/space/{authorize,pair}` — browser bridge pages
- `app/admin/AdminClient.js` — new **Filespaces** tab
- `middleware.js` — `api/desktop` + `api/space` excluded from the auth redirect

---

## 3. Filespaces & access (in Quest)

**Quest → Admin → Filespaces** → *New filespace* (name + bucket + key prefix, e.g.
`teams/marketing`). Add members by email with a role:
- **viewer** → read-only mount
- **editor / owner** → read-write
- Env-admins (`ADMIN_EMAILS`) implicitly access every filespace.

---

## 4. GitHub release repo + signing keys (enables auto-update)

The desktop app auto-updates from GitHub Releases at **`armraricky/armra-space`**.

### a. Create the repo and push
```bash
cd ~/Documents/s3-vault
git init && git add -A && git commit -m "ARMRA Space v0.1.0"
gh repo create armraricky/armra-space --private --source=. --remote=origin --push
```

### b. Add the signing key as repo secrets
A keypair was generated at `~/.armra-space/updater.key` (its public half is already
in `tauri.conf.json`). Add the private half as secrets:
```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.armra-space/updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body ""
```
> ⚠️ Back up `~/.armra-space/updater.key`. Lose it and you can't sign future
> updates — installed apps' auto-updater breaks. To rotate: `tauri signer generate`,
> replace `pubkey` in `tauri.conf.json`, ship one version manually, update the secret.

### c. Cut a release
```bash
./scripts/bump.sh patch
git commit -am "release vX.Y.Z" && git tag vX.Y.Z && git push && git push --tags
```
`.github/workflows/release.yml` builds aarch64, signs it, and publishes
`.dmg` + `.app.tar.gz` + `.sig` + `latest.json`. Installed apps update on next launch.

---

## Notes & limitations
- **Gatekeeper:** unsigned by Apple → first manual `.dmg` install needs right-click → **Open**. Auto-updates don't re-trigger it. Add Developer ID + notarization later without changing updater config.
- **aarch64 only** for now; Intel needs an `x86_64`/universal target in the workflow.
- **STS refresh:** creds last 1h. Re-open a filespace to refresh; background auto-refresh is a follow-up.
- **Windows deep links** need `tauri-plugin-single-instance`; the pairing-code fallback works everywhere today.
