use anyhow::{anyhow, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::Region;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub endpoint: Option<String>,
    pub prefix: Option<String>,
    // Present when the credentials are short-lived STS credentials minted for a
    // filespace (the normal path). None for long-lived manual keys.
    #[serde(default)]
    pub session_token: Option<String>,
    // S3 Transfer Acceleration — when on, rclone uses the s3-accelerate endpoint.
    #[serde(default)]
    pub accelerate: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct S3Entry {
    pub key: String,
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
}

pub async fn make_client(cfg: &S3Config) -> Result<Client> {
    let creds = Credentials::new(
        &cfg.access_key,
        &cfg.secret_key,
        cfg.session_token.clone(), // STS temp keys (ASIA…) are rejected without it
        None,
        "s3-vault",
    );

    let mut builder = aws_sdk_s3::config::Builder::new()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(cfg.region.clone()))
        .credentials_provider(creds);

    if let Some(ep) = &cfg.endpoint {
        builder = builder
            .endpoint_url(ep)
            .force_path_style(true);
    }

    Ok(Client::from_conf(builder.build()))
}

pub async fn list_objects(
    client: &Client,
    cfg: &S3Config,
    path: &str,
) -> Result<Vec<S3Entry>> {
    let prefix = build_prefix(cfg, path);
    let mut entries: Vec<S3Entry> = Vec::new();
    let mut dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut paginator = client
        .list_objects_v2()
        .bucket(&cfg.bucket)
        .prefix(&prefix)
        .delimiter("/")
        .into_paginator()
        .send();

    while let Some(page) = paginator.next().await {
        let page = page?;

        for cp in page.common_prefixes() {
            if let Some(p) = cp.prefix() {
                let rel = strip_base(p, &prefix);
                let name = rel.trim_end_matches('/').to_string();
                if !name.is_empty() && dirs.insert(name.clone()) {
                    entries.push(S3Entry {
                        key: p.to_string(),
                        name,
                        is_dir: true,
                        size: 0,
                        last_modified: None,
                        etag: None,
                    });
                }
            }
        }

        for obj in page.contents() {
            if let Some(key) = obj.key() {
                if key == prefix {
                    continue;
                }
                let name = strip_base(key, &prefix);
                if name.is_empty() || name.contains('/') {
                    continue;
                }
                entries.push(S3Entry {
                    key: key.to_string(),
                    name: name.to_string(),
                    is_dir: false,
                    size: obj.size().unwrap_or(0),
                    last_modified: obj
                        .last_modified()
                        .map(|t| t.fmt(aws_smithy_types::date_time::Format::DateTime).unwrap_or_default()),
                    etag: obj.e_tag().map(|e| e.trim_matches('"').to_string()),
                });
            }
        }
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(entries)
}

/// Recursively list every file object under a path (no delimiter), for pinning
/// a whole folder. Returns (full_key, size) pairs, skipping folder markers and
/// OS junk. Paginated, so it handles folders with thousands of files.
pub async fn list_objects_recursive(
    client: &Client,
    cfg: &S3Config,
    path: &str,
) -> Result<Vec<(String, i64)>> {
    let prefix = build_prefix(cfg, path);
    let mut out: Vec<(String, i64)> = Vec::new();

    let mut paginator = client
        .list_objects_v2()
        .bucket(&cfg.bucket)
        .prefix(&prefix)
        .into_paginator()
        .send();

    while let Some(page) = paginator.next().await {
        let page = page?;
        for obj in page.contents() {
            if let Some(key) = obj.key() {
                // Skip the folder marker itself and any sub-markers (keys ending in '/').
                if key.ends_with('/') {
                    continue;
                }
                let leaf = key.rsplit('/').next().unwrap_or(key);
                if leaf.is_empty() {
                    continue;
                }
                out.push((key.to_string(), obj.size().unwrap_or(0)));
            }
        }
    }
    Ok(out)
}

pub async fn download_object(
    client: &Client,
    bucket: &str,
    key: &str,
    dest: &Path,
) -> Result<String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).await?;
    }

    let resp = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;

    let etag = resp
        .e_tag()
        .map(|e| e.trim_matches('"').to_string())
        .unwrap_or_default();

    let body = resp.body.collect().await?;
    fs::write(dest, body.into_bytes()).await?;

    Ok(etag)
}

pub async fn head_object(client: &Client, bucket: &str, key: &str) -> Result<(i64, String)> {
    let resp = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;

    let size = resp.content_length().unwrap_or(0);
    let etag = resp
        .e_tag()
        .map(|e| e.trim_matches('"').to_string())
        .ok_or_else(|| anyhow!("no etag"))?;

    Ok((size, etag))
}

pub fn build_prefix(cfg: &S3Config, path: &str) -> String {
    let base = cfg.prefix.as_deref().unwrap_or("").trim_matches('/');
    let path = path.trim_matches('/');
    if base.is_empty() && path.is_empty() {
        String::new()
    } else if base.is_empty() {
        format!("{}/", path)
    } else if path.is_empty() {
        format!("{}/", base)
    } else {
        format!("{}/{}/", base, path)
    }
}

fn strip_base<'a>(key: &'a str, prefix: &str) -> &'a str {
    key.strip_prefix(prefix).unwrap_or(key)
}
