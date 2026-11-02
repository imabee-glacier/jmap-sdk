use crate::mail::Mode;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Store {
    db: Mutex<Connection>,
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
pub fn hash(value: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).unwrap_or_default())
    )
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Self::from_connection(Connection::open(path)?)
    }
    pub fn memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(db: Connection) -> Result<Self> {
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS intents (id TEXT PRIMARY KEY, scope TEXT NOT NULL, email TEXT NOT NULL, preview TEXT NOT NULL, digest TEXT NOT NULL, expires INTEGER NOT NULL, status TEXT NOT NULL, outcome TEXT, UNIQUE(scope,email));
            CREATE TABLE IF NOT EXISTS replies (scope TEXT NOT NULL, draft TEXT NOT NULL, source TEXT NOT NULL, PRIMARY KEY(scope,draft));
            CREATE TABLE IF NOT EXISTS policies (scope TEXT PRIMARY KEY, mode TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, timestamp INTEGER NOT NULL, scope TEXT NOT NULL, intent TEXT, event TEXT NOT NULL);")?;
        Ok(Self { db: Mutex::new(db) })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.db
            .lock()
            .map_err(|_| anyhow::anyhow!("State database lock failed"))
    }
    pub fn mode(&self, scope: &str) -> Result<Mode> {
        let mode: Option<String> = self
            .lock()?
            .query_row("SELECT mode FROM policies WHERE scope=?", [scope], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(if mode.as_deref() == Some("autonomous") {
            Mode::Autonomous
        } else {
            Mode::Manual
        })
    }
    pub fn set_mode(&self, scope: &str, mode: Mode) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let name = if mode == Mode::Manual {
            "manual"
        } else {
            "autonomous"
        };
        tx.execute(
            "INSERT INTO policies VALUES (?,?) ON CONFLICT(scope) DO UPDATE SET mode=excluded.mode",
            params![scope, name],
        )?;
        tx.execute(
            "INSERT INTO events(timestamp,scope,event) VALUES (?,?,?)",
            params![now(), scope, format!("mode:{name}")],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn prepare(&self, scope: &str, preview: Value) -> Result<Value> {
        let email = preview["id"].as_str().context("Missing email id")?;
        let digest = hash(&preview);
        let id = uuid::Uuid::new_v4().to_string();
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        // A cancelled intent never reached the server; let the same draft be reviewed again.
        tx.execute(
            "UPDATE intents SET preview=?,digest=?,expires=?,status='prepared',outcome=NULL WHERE scope=? AND email=? AND status='cancelled'",
            params![preview.to_string(), digest, now() + 900, scope, email],
        )?;
        tx.execute("INSERT INTO intents VALUES (?,?,?,?,?,?, 'prepared',NULL) ON CONFLICT(scope,email) DO NOTHING", params![id,scope,email,preview.to_string(),digest,now()+900])?;
        let (existing, old_digest): (String, String) = tx.query_row(
            "SELECT id,digest FROM intents WHERE scope=? AND email=?",
            params![scope, email],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        ensure!(
            old_digest == digest,
            "An intent already exists for different draft contents or identity; create a new draft"
        );
        tx.execute(
            "UPDATE intents SET expires=? WHERE id=? AND status='prepared'",
            params![now() + 900, existing],
        )?;
        tx.commit()?;
        drop(db);
        self.get(scope, &existing)
    }
    pub fn get(&self, scope: &str, id: &str) -> Result<Value> {
        let (preview, digest, expires, status, outcome): (
            String,
            String,
            i64,
            String,
            Option<String>,
        ) = self
            .lock()?
            .query_row(
                "SELECT preview,digest,expires,status,outcome FROM intents WHERE scope=? AND id=?",
                params![scope, id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
            .context("Send intent not found")?;
        Ok(
            json!({"intent_id":id,"preview":serde_json::from_str::<Value>(&preview)?,"preview_hash":digest,"expires_at":expires,"status":status,"outcome":outcome.map(|s| serde_json::from_str::<Value>(&s)).transpose()?}),
        )
    }
    pub fn claim(
        &self,
        scope: &str,
        id: &str,
        confirmed: bool,
        expected_hash: Option<&str>,
    ) -> Result<bool> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let (digest, expires, status): (String, i64, String) = tx.query_row(
            "SELECT digest,expires,status FROM intents WHERE scope=? AND id=?",
            params![scope, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if status != "prepared" {
            return Ok(false);
        }
        ensure!(
            expires > now(),
            "Preview expired; prepare_send again and review the new preview"
        );
        let mode: Option<String> = tx
            .query_row("SELECT mode FROM policies WHERE scope=?", [scope], |r| {
                r.get(0)
            })
            .optional()?;
        if mode.as_deref() != Some("autonomous") {
            ensure!(
                confirmed && expected_hash == Some(digest.as_str()),
                "Manual mode requires user confirmation and the exact preview_hash from prepare_send"
            );
        }
        let claimed = tx.execute(
            "UPDATE intents SET status='submitting' WHERE scope=? AND id=? AND status='prepared'",
            params![scope, id],
        )? == 1;
        if claimed {
            tx.execute(
                "INSERT INTO events(timestamp,scope,intent,event) VALUES (?,?,?,'submitting')",
                params![now(), scope, id],
            )?;
        }
        tx.commit()?;
        Ok(claimed)
    }
    pub fn finish(&self, scope: &str, id: &str, status: &str, outcome: Value) -> Result<()> {
        ensure!(
            ["submitted", "uncertain", "rejected"].contains(&status),
            "Invalid terminal state"
        );
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        ensure!(tx.execute("UPDATE intents SET status=?,outcome=? WHERE scope=? AND id=? AND status='submitting'", params![status,outcome.to_string(),scope,id])? == 1, "Intent is not submitting");
        tx.execute(
            "INSERT INTO events(timestamp,scope,intent,event) VALUES (?,?,?,?)",
            params![now(), scope, id, status],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn cancel(&self, scope: &str, id: &str) -> Result<Value> {
        self.lock()?.execute(
            "UPDATE intents SET status='cancelled' WHERE scope=? AND id=? AND status='prepared'",
            params![scope, id],
        )?;
        self.get(scope, id)
    }
    pub fn remember_reply(&self, scope: &str, draft: &str, source: &str) -> Result<()> {
        self.lock()?.execute(
            "INSERT OR REPLACE INTO replies VALUES (?,?,?)",
            params![scope, draft, source],
        )?;
        Ok(())
    }
    pub fn reply_source(&self, scope: &str, draft: &str) -> Result<Option<String>> {
        Ok(self
            .lock()?
            .query_row(
                "SELECT source FROM replies WHERE scope=? AND draft=?",
                params![scope, draft],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn events(&self, scope: &str) -> Result<Value> {
        let db = self.lock()?;
        let mut stmt = db.prepare(
            "SELECT timestamp,intent,event FROM events WHERE scope=? ORDER BY id DESC LIMIT 100",
        )?;
        let rows = stmt.query_map([scope], |r| Ok(json!({"timestamp":r.get::<_,i64>(0)?,"intent_id":r.get::<_,Option<String>>(1)?,"event":r.get::<_,String>(2)?})))?;
        Ok(json!({"events":rows.collect::<rusqlite::Result<Vec<_>>>()?}))
    }
}
