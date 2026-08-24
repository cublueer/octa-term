//! 计算历史：SQLite(WAL) 落盘，每次求值完成即写入并依赖 WAL 的
//! autocheckpoint + 每插入一行的 commit 语义保证断电不丢（fsync 交给
//! SQLite 默认 synchronous=FULL 的 WAL 模式）。
//!
//! 冷调用与 daemon（阶段 5）共用同一张表，靠 WAL 与 busy_timeout 并发共存。

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

pub const MODE_COLD: &str = "cold";
pub const MODE_DAEMON: &str = "daemon";

pub const STATUS_OK: &str = "ok";
pub const STATUS_ERROR: &str = "error";
pub const STATUS_TIMEOUT: &str = "timeout";
pub const STATUS_BLOCKED: &str = "blocked";

pub struct HistoryDb {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub id: i64,
    pub ts: i64,
    pub expr: String,
    pub result: String,
    pub duration_ms: i64,
    pub mode: String,
    pub status: String,
}

impl HistoryDb {
    /// 打开（必要时创建）历史库。库文件权限强制 0600，目录 0700。
    pub fn open(path: &Path) -> Result<HistoryDb> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)?;
            }
            let file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .with_context(|| format!("creating history db {}", path.display()))?;
            drop(file);
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening history db {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                ts          INTEGER NOT NULL,
                expr        TEXT    NOT NULL,
                result      TEXT    NOT NULL DEFAULT '',
                duration_ms INTEGER,
                mode        TEXT    NOT NULL DEFAULT 'cold',
                status      TEXT    NOT NULL DEFAULT 'ok'
            );
            CREATE INDEX IF NOT EXISTS idx_history_ts ON history(ts);",
        )?;
        Ok(HistoryDb { conn })
    }

    pub fn insert(
        &self,
        ts: i64,
        expr: &str,
        result: &str,
        duration_ms: Option<i64>,
        mode: &str,
        status: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO history (ts, expr, result, duration_ms, mode, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![ts, expr, result, duration_ms, mode, status],
        )?;
        Ok(())
    }

    pub fn list(&self, limit: usize, grep: Option<&str>, since_ts: i64) -> Result<Vec<Row>> {
        let limit = limit.clamp(1, 10_000) as i64;
        let sql = match grep {
            Some(_) => {
                "SELECT id, ts, expr, result, duration_ms, mode, status
                 FROM history
                 WHERE ts >= ?1 AND expr LIKE ?2
                 ORDER BY id DESC LIMIT ?3"
            }
            None => {
                "SELECT id, ts, expr, result, duration_ms, mode, status
                 FROM history
                 WHERE ts >= ?1
                 ORDER BY id DESC LIMIT ?2"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = match grep {
            Some(needle) => stmt.query_map(
                rusqlite::params![since_ts, format!("%{needle}%"), limit],
                row_to_entry,
            )?,
            None => stmt.query_map(rusqlite::params![since_ts, limit], row_to_entry)?,
        };
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn clear(&self) -> Result<usize> {
        let count = self.conn.execute("DELETE FROM history", [])?;
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(count)
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get(0)?,
        ts: row.get(1)?,
        expr: row.get(2)?,
        result: row.get(3)?,
        duration_ms: row.get(4)?,
        mode: row.get(5)?,
        status: row.get(6)?,
    })
}
