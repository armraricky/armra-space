use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PinnedFile {
    pub id: String,
    pub s3_key: String,
    pub bucket: String,
    pub local_path: String,
    pub size: i64,
    pub last_synced: Option<DateTime<Utc>>,
    pub is_cached: bool,
    pub etag: Option<String>,
}

pub fn open(db_path: &PathBuf) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pins (
            id          TEXT PRIMARY KEY,
            s3_key      TEXT NOT NULL,
            bucket      TEXT NOT NULL,
            local_path  TEXT NOT NULL,
            size        INTEGER NOT NULL DEFAULT 0,
            last_synced TEXT,
            is_cached   INTEGER NOT NULL DEFAULT 0,
            etag        TEXT,
            UNIQUE(bucket, s3_key)
        );
        CREATE TABLE IF NOT EXISTS config (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS listing_cache (
            cache_key TEXT PRIMARY KEY,
            json      TEXT NOT NULL,
            cached_at INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

/// Cache a directory listing (JSON of Vec<S3Entry>) so the browse tab can paint
/// instantly from the last-known tree before the live S3 listing returns.
pub fn set_listing_cache(conn: &Connection, key: &str, json: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO listing_cache (cache_key, json, cached_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(cache_key) DO UPDATE SET json = excluded.json, cached_at = excluded.cached_at",
        params![key, json, Utc::now().timestamp_millis()],
    )?;
    Ok(())
}

/// Return a cached listing's JSON if present (ignoring age — the caller refreshes live).
pub fn get_listing_cache(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT json FROM listing_cache WHERE cache_key = ?1")?;
    let mut rows = stmt.query(params![key])?;
    if let Some(row) = rows.next()? { Ok(Some(row.get(0)?)) } else { Ok(None) }
}

pub fn upsert_pin(conn: &Connection, pin: &PinnedFile) -> Result<()> {
    conn.execute(
        "INSERT INTO pins (id, s3_key, bucket, local_path, size, last_synced, is_cached, etag)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(bucket, s3_key) DO UPDATE SET
           local_path = excluded.local_path,
           size = excluded.size,
           last_synced = excluded.last_synced,
           is_cached = excluded.is_cached,
           etag = excluded.etag",
        params![
            pin.id,
            pin.s3_key,
            pin.bucket,
            pin.local_path,
            pin.size,
            pin.last_synced.map(|t| t.to_rfc3339()),
            pin.is_cached as i32,
            pin.etag,
        ],
    )?;
    Ok(())
}

pub fn delete_pin(conn: &Connection, bucket: &str, s3_key: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM pins WHERE bucket = ?1 AND s3_key = ?2",
        params![bucket, s3_key],
    )?;
    Ok(())
}

pub fn list_pins(conn: &Connection) -> Result<Vec<PinnedFile>> {
    let mut stmt = conn.prepare(
        "SELECT id, s3_key, bucket, local_path, size, last_synced, is_cached, etag FROM pins",
    )?;
    let pins = stmt
        .query_map([], |row| {
            let last_synced_str: Option<String> = row.get(5)?;
            let last_synced = last_synced_str
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            Ok(PinnedFile {
                id: row.get(0)?,
                s3_key: row.get(1)?,
                bucket: row.get(2)?,
                local_path: row.get(3)?,
                size: row.get(4)?,
                last_synced,
                is_cached: row.get::<_, i32>(6)? != 0,
                etag: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pins)
}

pub fn mark_cached(conn: &Connection, bucket: &str, s3_key: &str, etag: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE pins SET is_cached = 1, last_synced = ?1, etag = ?2 WHERE bucket = ?3 AND s3_key = ?4",
        params![
            Utc::now().to_rfc3339(),
            etag,
            bucket,
            s3_key,
        ],
    )?;
    Ok(())
}

pub fn set_config(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_config(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM config WHERE key = ?1")?;
    let mut rows = stmt.query(params![key])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}
