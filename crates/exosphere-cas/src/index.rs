// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! SQLite metadata index for CAS objects. Object bytes remain filesystem-owned.
use rusqlite::{Connection, params};
use std::path::Path;
pub struct CasIndex {
    conn: Connection,
}
impl CasIndex {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS objects(hash TEXT PRIMARY KEY,size INTEGER NOT NULL,created_at INTEGER NOT NULL DEFAULT (unixepoch()));")?;
        Ok(Self { conn })
    }
    pub fn record(&self, hash: &str, size: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO objects(hash,size) VALUES(?1,?2)",
            params![hash, size],
        )?;
        Ok(())
    }
}
