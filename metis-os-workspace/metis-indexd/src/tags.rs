//! SQLite user-tag sidecar (`$XDG_DATA_HOME/metis/search-tags.sqlite`).

use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::categorize::normalize_tag;

pub struct TagStore {
    conn: Mutex<Connection>,
}

impl TagStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS user_tags (
               path TEXT NOT NULL,
               dev INTEGER NOT NULL DEFAULT 0,
               ino INTEGER NOT NULL DEFAULT 0,
               tag TEXT NOT NULL,
               PRIMARY KEY (path, tag)
             );
             CREATE INDEX IF NOT EXISTS idx_user_tags_dev_ino ON user_tags(dev, ino);
             CREATE INDEX IF NOT EXISTS idx_user_tags_tag ON user_tags(tag);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn tags_for_path(&self, path: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("tag store lock poisoned".into()),
            )
        })?;
        let mut stmt = conn.prepare("SELECT tag FROM user_tags WHERE path = ?1 ORDER BY tag")?;
        let rows = stmt.query_map(params![path], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn tags_for_inode(&self, dev: u64, ino: u64) -> rusqlite::Result<Vec<String>> {
        if dev == 0 && ino == 0 {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().map_err(|_| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("tag store lock poisoned".into()),
            )
        })?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT tag FROM user_tags WHERE dev = ?1 AND ino = ?2 ORDER BY tag",
        )?;
        let rows = stmt.query_map(params![dev as i64, ino as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn add_tag(&self, path: &str, dev: u64, ino: u64, tag: &str) -> rusqlite::Result<()> {
        let tag = normalize_tag(tag);
        if tag.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|_| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("tag store lock poisoned".into()),
            )
        })?;
        conn.execute(
            "INSERT OR IGNORE INTO user_tags(path, dev, ino, tag) VALUES(?1, ?2, ?3, ?4)",
            params![path, dev as i64, ino as i64, tag],
        )?;
        Ok(())
    }

    pub fn remove_tag(&self, path: &str, tag: &str) -> rusqlite::Result<()> {
        let tag = normalize_tag(tag);
        let conn = self.conn.lock().map_err(|_| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("tag store lock poisoned".into()),
            )
        })?;
        conn.execute(
            "DELETE FROM user_tags WHERE path = ?1 AND tag = ?2",
            params![path, tag],
        )?;
        Ok(())
    }

    pub fn suggest(&self, prefix: &str, limit: u32) -> rusqlite::Result<Vec<String>> {
        let prefix = normalize_tag(prefix);
        let conn = self.conn.lock().map_err(|_| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("tag store lock poisoned".into()),
            )
        })?;
        let mut stmt = if prefix.is_empty() {
            conn.prepare(
                "SELECT tag, COUNT(*) AS c FROM user_tags GROUP BY tag ORDER BY c DESC, tag LIMIT ?1",
            )?
        } else {
            conn.prepare(
                "SELECT tag, COUNT(*) AS c FROM user_tags WHERE tag LIKE ?1 ESCAPE '\\' GROUP BY tag ORDER BY c DESC, tag LIMIT ?2",
            )?
        };
        let limit = limit.clamp(1, 200) as i64;
        let rows = if prefix.is_empty() {
            stmt.query_map(params![limit], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let like = format!("{}%", escape_like(&prefix));
            stmt.query_map(params![like, limit], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    /// Resolve user tags preferring inode match, falling back to path.
    pub fn user_tags_for(&self, path: &str, dev: u64, ino: u64) -> Vec<String> {
        if let Ok(tags) = self.tags_for_inode(dev, ino)
            && !tags.is_empty()
        {
            return tags;
        }
        self.tags_for_path(path).unwrap_or_default()
    }
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub fn default_tags_db_path() -> PathBuf {
    let base = directories::BaseDirs::new()
        .map(|b| b.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("metis").join("search-tags.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn add_list_remove() {
        let dir = tempdir().unwrap();
        let store = TagStore::open(&dir.path().join("t.sqlite")).unwrap();
        store.add_tag("/tmp/a.rs", 1, 2, "Work").unwrap();
        store.add_tag("/tmp/a.rs", 1, 2, "urgent").unwrap();
        let tags = store.tags_for_path("/tmp/a.rs").unwrap();
        assert_eq!(tags, vec!["urgent".to_string(), "work".to_string()]);
        store.remove_tag("/tmp/a.rs", "work").unwrap();
        assert_eq!(store.tags_for_path("/tmp/a.rs").unwrap(), vec!["urgent"]);
        assert!(store.suggest("u", 10).unwrap().contains(&"urgent".into()));
    }
}
