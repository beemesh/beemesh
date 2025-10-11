use anyhow::Result;
use log::info;
use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Keystore {
    conn: Connection,
}

impl Keystore {
    /// Open a keystore at the given path. If `ephemeral` is true or path is
    /// ":memory:", an in-memory sqlite DB will be used.
    pub fn open(path: &Path, ephemeral: bool) -> Result<Self> {
        let conn = if ephemeral || path.to_str() == Some(":memory:") {
            // If the caller requested an ephemeral keystore but wants a shared in-memory
            // database across multiple connections in the same process (useful for
            // tests that open the keystore multiple times), they can set
            // BEEMESH_KEYSTORE_SHARED_NAME to a stable name. This will open an
            // SQLite URI like "file:<name>?mode=memory&cache=shared" with
            // SQLITE_OPEN_URI so multiple connections with the same name share the
            // same in-memory database.
            if let Ok(shared_name) = std::env::var("BEEMESH_KEYSTORE_SHARED_NAME") {
                // Try temp file approach instead of pure in-memory with cache=shared
                let temp_path =
                    std::env::temp_dir().join(format!("beemesh_keystore_{}", shared_name));
                info!(
                    "Keystore::open using shared temp file: {:?} thread_id={:?}",
                    temp_path,
                    std::thread::current().id()
                );
                let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_FULL_MUTEX;
                let conn = Connection::open_with_flags(&temp_path, flags)?;
                info!(
                    "Keystore::open connection established for thread {:?}",
                    std::thread::current().id()
                );
                conn
            } else {
                info!("Keystore::open using private in-memory database (:memory:)");
                Connection::open_in_memory()?
            }
        } else {
            // Ensure parent dir exists
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX;
            info!("Keystore::open using file path: {}", path.display());
            Connection::open_with_flags(path, flags)?
        };

        // Initialize schema
        conn.execute_batch(
            "BEGIN;
            CREATE TABLE IF NOT EXISTS shares (
                cid TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                blob BLOB NOT NULL,
                meta TEXT
            );
            COMMIT;",
        )?;

        // Test if this is truly a shared connection by trying to read any existing data
        let existing_count: i64 = conn
            .prepare("SELECT COUNT(*) FROM shares")?
            .query_row([], |row| row.get(0))?;
        info!(
            "Keystore::open initialized with {} existing records thread_id={:?}",
            existing_count,
            std::thread::current().id()
        );

        Ok(Self { conn })
    }

    /// Put an encrypted share blob keyed by cid. Idempotent (INSERT OR REPLACE).
    pub fn put(&self, cid: &str, blob: &[u8], meta: Option<&str>) -> Result<()> {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
        self.conn.execute(
            "INSERT OR REPLACE INTO shares (cid, created_at, blob, meta) VALUES (?1, ?2, ?3, ?4)",
            params![cid, ts, blob, meta],
        )?;
        Ok(())
    }

    /// Get the encrypted blob for a given cid, if present.
    pub fn get(&self, cid: &str) -> Result<Option<Vec<u8>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT blob FROM shares WHERE cid = ?1")?;
        let mut rows = stmt.query([cid])?;
        if let Some(row) = rows.next()? {
            let blob: Vec<u8> = row.get(0)?;
            Ok(Some(blob))
        } else {
            Ok(None)
        }
    }

    /// List all CIDs stored in the keystore
    pub fn list_cids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT cid FROM shares")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let cid: String = row.get(0)?;
            out.push(cid);
        }
        Ok(out)
    }

    /// List all entries with their metadata
    pub fn list_entries_with_metadata(&self) -> Result<Vec<serde_json::Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT cid, meta, created_at FROM shares ORDER BY created_at")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let cid: String = row.get(0)?;
            let meta: Option<String> = row.get(1)?;
            let created_at: i64 = row.get(2)?;

            let entry = serde_json::json!({
                "cid": cid,
                "meta": meta,
                "created_at": created_at,
                "type": if let Some(ref m) = meta {
                    if m.starts_with("capability:") {
                        "capability"
                    } else {
                        "keyshare"
                    }
                } else {
                    "unknown"
                }
            });
            out.push(entry);
        }
        Ok(out)
    }

    /// Find CID for a share with the given manifest_id in its metadata
    pub fn find_cid_for_manifest(&self, manifest_id: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT cid FROM shares WHERE meta = ?1")?;
        log::info!(
            "Keystore::find_cid_for_manifest looking up meta='{}'",
            manifest_id
        );
        let mut rows = stmt.query([manifest_id])?;
        if let Some(row) = rows.next()? {
            let cid: String = row.get(0)?;
            log::info!(
                "Keystore::find_cid_for_manifest found cid='{}' for meta='{}'",
                cid,
                manifest_id
            );
            Ok(Some(cid))
        } else {
            log::info!(
                "Keystore::find_cid_for_manifest no cid found for meta='{}'",
                manifest_id
            );
            Ok(None)
        }
    }

    /// Find all CIDs for shares with the given manifest_id in their metadata
    pub fn find_cids_for_manifest(&self, manifest_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT cid FROM shares WHERE meta = ?1")?;
        log::info!(
            "Keystore::find_cids_for_manifest looking up meta='{}'",
            manifest_id
        );
        let mut rows = stmt.query([manifest_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let cid: String = row.get(0)?;
            out.push(cid);
        }
        log::info!(
            "Keystore::find_cids_for_manifest found {} cids for meta='{}'",
            out.len(),
            manifest_id
        );
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_put_get_memory() {
        let p = PathBuf::from(":memory:");
        let ks = Keystore::open(&p, true).expect("open mem");
        let cid = "deadbeef";
        let blob = b"hello world";
        ks.put(cid, blob, Some("meta")).expect("put");
        let got = ks.get(cid).expect("get").expect("exists");
        assert_eq!(got, blob);
    }
}
