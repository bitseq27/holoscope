//! hs-cache — disk cache: SQLite index + SHA-256-named blob store (plan §13, App. C).
//!
//! M1 scope: geocode cache (forever, per NFR-07), raw source blobs (COG tile
//! payloads), region manifest kv. Node/zstd tiers arrive with M3.

use anyhow::{Context, Result};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Cache {
    db: Mutex<Connection>,
    root: PathBuf,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Cache {
    /// `root` e.g. %LOCALAPPDATA%\HoloScope\cache or ~/.cache/holoscope
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root.join("raw")).ok();
        std::fs::create_dir_all(root.join("db")).ok();
        let db = Connection::open(root.join("db").join("cache.db"))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS geocode (
                key TEXT PRIMARY KEY,
                json TEXT NOT NULL,
                ts INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS blob (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                bytes INTEGER NOT NULL,
                ts INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS kv (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );",
        )?;
        Ok(Self {
            db: Mutex::new(db),
            root: root.to_path_buf(),
        })
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    // ------------------------------------------------------------ geocode
    pub fn geocode_get(&self, kind: &str, query: &str) -> Result<Option<String>> {
        let key = format!("{kind}:{}", query.to_lowercase());
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT json FROM geocode WHERE key = ?1")?;
        let mut rows = st.query([&key])?;
        match rows.next()? {
            Some(r) => Ok(Some(r.get::<_, String>(0)?)),
            None => Ok(None),
        }
    }

    pub fn geocode_put(&self, kind: &str, query: &str, json: &str) -> Result<()> {
        let key = format!("{kind}:{}", query.to_lowercase());
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO geocode (key, json, ts) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, json, Self::now()],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------ blobs
    /// Store bytes under their content hash; returns the id (sha256 hex).
    pub fn blob_put(&self, kind: &str, bytes: &[u8]) -> Result<String> {
        let id = sha256_hex(bytes);
        let path = self.blob_path(&id);
        if !path.exists() {
            let tmp = path.with_extension("part");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, &path)?;
        }
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO blob (id, kind, bytes, ts) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, kind, bytes.len() as i64, Self::now()],
        )?;
        Ok(id)
    }

    /// Read a blob, verifying its content hash (plan §13 corrupt-cache path).
    /// Returns `None` (and quarantines) when the bytes do not match the id.
    pub fn blob_get(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let path = self.blob_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        if sha256_hex(&bytes) != id {
            self.quarantine(&path, id);
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    fn quarantine(&self, path: &std::path::Path, id: &str) {
        let qdir = self.root.join("raw").join("quarantine");
        std::fs::create_dir_all(&qdir).ok();
        let _ = std::fs::rename(path, qdir.join(format!("{id}.bin")));
        let db = self.db.lock().unwrap();
        let _ = db.execute("DELETE FROM blob WHERE id = ?1", rusqlite::params![id]);
    }

    pub fn blob_path(&self, id: &str) -> PathBuf {
        // two-char prefix dirs keep readdir cheap
        let (a, b) = id.split_at(2.min(id.len()));
        let dir = self.root.join("raw").join(a);
        std::fs::create_dir_all(&dir).ok();
        dir.join(format!("{b}.bin"))
    }

    /// Fetch-through helper: cache-first (verified), else fetch then store.
    /// A corrupt cached blob is quarantined and re-fetched once (plan §13).
    pub fn blob_fetch(&self, kind: &str, key: &str, fetch: impl FnOnce() -> Result<Vec<u8>>) -> Result<Vec<u8>> {
        // logical key → content id (recorded on first download)
        if let Some(id) = self.kv_get(&format!("blobkey:{key}"))? {
            if let Some(b) = self.blob_get(&id)? {
                return Ok(b);
            }
            // cached bytes were corrupt+quarantined: drop the logical mapping
            let db = self.db.lock().unwrap();
            let _ = db.execute(
                "DELETE FROM kv WHERE key = ?1",
                rusqlite::params![format!("blobkey:{key}")],
            );
        }
        let bytes = fetch().with_context(|| format!("fetch {key}"))?;
        // verify integrity once downloaded
        let got = sha256_hex(&bytes);
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO blob (id, kind, bytes, ts) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![got, kind, bytes.len() as i64, Self::now()],
        )?;
        drop(db);
        // atomic write: an interrupted download must never leave a corrupt
        // blob behind (BUG-M6.5-2: interrupted session poisoned the overpass
        // cache; quarantine caught it, but prevent instead of cure)
        let path = self.blob_path(&got);
        if !path.exists() {
            let tmp = path.with_extension("part");
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &path)?;
        }
        // also record the logical key → content id mapping
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
            rusqlite::params![format!("blobkey:{key}"), got],
        )?;
        Ok(bytes)
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT value FROM kv WHERE key = ?1")?;
        let mut rows = st.query([key])?;
        match rows.next()? {
            Some(r) => Ok(Some(r.get::<_, String>(0)?)),
            None => Ok(None),
        }
    }

    pub fn kv_put(&self, key: &str, value: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_roundtrip_and_kv() {
        let dir = std::env::temp_dir().join(format!("hs-cache-test-{}", std::process::id()));
        let c = Cache::open(&dir).unwrap();
        let id = c.blob_put("test", b"hello holo").unwrap();
        assert_eq!(c.blob_get(&id).unwrap().unwrap(), b"hello holo");
        let fetched = c
            .blob_fetch("test", "logical-key", || Ok(b"payload".to_vec()))
            .unwrap();
        assert_eq!(fetched, b"payload");
        // second call must hit the cache (closure would panic if called)
        let again = c
            .blob_fetch("test", "logical-key", || panic!("should be cached"))
            .unwrap();
        assert_eq!(again, b"payload");
        c.kv_put("manifest:x", "{\"a\":1}").unwrap();
        assert_eq!(c.kv_get("manifest:x").unwrap().unwrap(), "{\"a\":1}");
        c.geocode_put("photon", "Kyoto", "{\"v\":1}").unwrap();
        assert_eq!(
            c.geocode_get("photon", "kyoto").unwrap().unwrap(),
            "{\"v\":1}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_blob_quarantined_and_refetched() {
        let dir = std::env::temp_dir().join(format!("hs-cache-q-{}", std::process::id()));
        let c = Cache::open(&dir).unwrap();
        let id = c.blob_put("t", b"good bytes").unwrap();
        // corrupt the file behind the cache's back
        let p = c.blob_path(&id);
        std::fs::write(&p, b"EVIL bytes").unwrap();
        assert!(c.blob_get(&id).unwrap().is_none(), "corrupt blob must not verify");
        assert!(c.blob_path(&id).exists() == false, "corrupt blob must be quarantined");
        let q = dir.join("raw").join("quarantine").join(format!("{id}.bin"));
        assert!(q.exists(), "quarantine copy expected");
        // fetch-through recovers transparently
        let got = c
            .blob_fetch("t", "logical", || Ok(b"good bytes".to_vec()))
            .unwrap();
        assert_eq!(got, b"good bytes");
        std::fs::remove_dir_all(&dir).ok();
    }
}
