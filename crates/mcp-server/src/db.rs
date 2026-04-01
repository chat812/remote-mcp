use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Capabilities {
    pub version: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub hostname: Option<String>,
    pub has_systemd: bool,
    pub has_docker: bool,
    pub has_git: bool,
    #[serde(default)]
    pub has_ui_automation: bool,
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub id: String,
    pub label: String,
    pub host: String,
    pub port: i64,
    pub os: String,
    pub transport: String,
    pub ssh_user: Option<String>,
    pub ssh_key_path: Option<String>,
    pub ssh_password: Option<String>,
    pub agent_url: Option<String>,
    pub agent_token: Option<String>,
    pub capabilities: Option<Capabilities>,
    pub last_seen: Option<i64>,
    pub status: String,
    pub created_at: i64,
}

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

fn db_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("remote-exec-mcp")
        .join("machines.db")
}

impl Db {
    pub fn open() -> Result<Self> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("Failed to open DB at {}", path.display()))?;

        // Pragmas
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )?;

        // Create table
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS machines (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                host TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 22,
                os TEXT NOT NULL DEFAULT 'linux',
                transport TEXT NOT NULL DEFAULT 'ssh',
                ssh_user TEXT,
                ssh_key_path TEXT,
                ssh_password TEXT,
                agent_url TEXT,
                agent_token TEXT,
                capabilities TEXT,
                last_seen INTEGER,
                status TEXT DEFAULT 'unknown',
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<Machine>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, label, host, port, os, transport, ssh_user, ssh_key_path, ssh_password,
                    agent_url, agent_token, capabilities, last_seen, status, created_at
             FROM machines WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_machine(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list(&self) -> Result<Vec<Machine>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, label, host, port, os, transport, ssh_user, ssh_key_path, ssh_password,
                    agent_url, agent_token, capabilities, last_seen, status, created_at
             FROM machines ORDER BY label",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(row_to_machine_sync(row))
        })?;
        let mut machines = Vec::new();
        for row in rows {
            machines.push(row??);
        }
        Ok(machines)
    }

    pub fn upsert(&self, m: &Machine) -> Result<()> {
        let caps_json = m.capabilities.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO machines (id, label, host, port, os, transport, ssh_user, ssh_key_path, ssh_password,
                agent_url, agent_token, capabilities, last_seen, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
                label=excluded.label, host=excluded.host, port=excluded.port,
                os=excluded.os, transport=excluded.transport, ssh_user=excluded.ssh_user,
                ssh_key_path=excluded.ssh_key_path, ssh_password=excluded.ssh_password,
                agent_url=excluded.agent_url, agent_token=excluded.agent_token,
                capabilities=excluded.capabilities, last_seen=excluded.last_seen,
                status=excluded.status",
            params![
                m.id, m.label, m.host, m.port, m.os, m.transport,
                m.ssh_user, m.ssh_key_path, m.ssh_password,
                m.agent_url, m.agent_token, caps_json,
                m.last_seen, m.status, m.created_at
            ],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM machines WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn update_heartbeat(&self, id: &str, status: &str, last_seen: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE machines SET status = ?1, last_seen = ?2 WHERE id = ?3",
            params![status, last_seen, id],
        )?;
        Ok(())
    }

    pub fn update_capabilities(&self, id: &str, caps: &Capabilities) -> Result<()> {
        let caps_json = serde_json::to_string(caps)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE machines SET capabilities = ?1 WHERE id = ?2",
            params![caps_json, id],
        )?;
        Ok(())
    }

    pub fn maintenance(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA optimize; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

fn row_to_machine(row: &rusqlite::Row<'_>) -> Result<Machine, rusqlite::Error> {
    let caps_str: Option<String> = row.get(11)?;
    let capabilities = caps_str.and_then(|s| serde_json::from_str(&s).ok());
    Ok(Machine {
        id: row.get(0)?,
        label: row.get(1)?,
        host: row.get(2)?,
        port: row.get(3)?,
        os: row.get(4)?,
        transport: row.get(5)?,
        ssh_user: row.get(6)?,
        ssh_key_path: row.get(7)?,
        ssh_password: row.get(8)?,
        agent_url: row.get(9)?,
        agent_token: row.get(10)?,
        capabilities,
        last_seen: row.get(12)?,
        status: row.get::<_, Option<String>>(13)?.unwrap_or_else(|| "unknown".to_string()),
        created_at: row.get(14)?,
    })
}

fn row_to_machine_sync(row: &rusqlite::Row<'_>) -> Result<Machine, rusqlite::Error> {
    row_to_machine(row)
}

#[cfg(test)]
impl Db {
    /// Open an in-memory SQLite database for testing.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS machines (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                host TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 22,
                os TEXT NOT NULL DEFAULT 'linux',
                transport TEXT NOT NULL DEFAULT 'ssh',
                ssh_user TEXT,
                ssh_key_path TEXT,
                ssh_password TEXT,
                agent_url TEXT,
                agent_token TEXT,
                capabilities TEXT,
                last_seen INTEGER,
                status TEXT DEFAULT 'unknown',
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );",
        )?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_machine(id: &str, label: &str) -> Machine {
        Machine {
            id: id.to_string(),
            label: label.to_string(),
            host: "192.168.1.1".to_string(),
            port: 22,
            os: "linux".to_string(),
            transport: "ssh".to_string(),
            ssh_user: Some("root".to_string()),
            ssh_key_path: Some("~/.ssh/id_rsa".to_string()),
            ssh_password: None,
            agent_url: None,
            agent_token: None,
            capabilities: None,
            last_seen: None,
            status: "unknown".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    #[test]
    fn upsert_and_get() {
        let db = Db::open_in_memory().unwrap();
        let m = sample_machine("m1", "prod");
        db.upsert(&m).unwrap();
        let got = db.get("m1").unwrap().unwrap();
        assert_eq!(got.id, "m1");
        assert_eq!(got.label, "prod");
        assert_eq!(got.host, "192.168.1.1");
        assert_eq!(got.port, 22);
        assert_eq!(got.os, "linux");
        assert_eq!(got.transport, "ssh");
        assert_eq!(got.ssh_user, Some("root".to_string()));
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.get("nonexistent").unwrap().is_none());
    }

    #[test]
    fn list_returns_all_machines() {
        let db = Db::open_in_memory().unwrap();
        db.upsert(&sample_machine("a", "alpha")).unwrap();
        db.upsert(&sample_machine("b", "beta")).unwrap();
        db.upsert(&sample_machine("c", "gamma")).unwrap();
        let list = db.list().unwrap();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn list_empty_db() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.list().unwrap().is_empty());
    }

    #[test]
    fn delete_existing_machine() {
        let db = Db::open_in_memory().unwrap();
        db.upsert(&sample_machine("x", "to-delete")).unwrap();
        assert!(db.delete("x").unwrap());
        assert!(db.get("x").unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let db = Db::open_in_memory().unwrap();
        assert!(!db.delete("ghost").unwrap());
    }

    #[test]
    fn upsert_updates_existing() {
        let db = Db::open_in_memory().unwrap();
        let mut m = sample_machine("m1", "original");
        db.upsert(&m).unwrap();
        m.label = "updated".to_string();
        m.host = "10.0.0.1".to_string();
        db.upsert(&m).unwrap();
        let got = db.get("m1").unwrap().unwrap();
        assert_eq!(got.label, "updated");
        assert_eq!(got.host, "10.0.0.1");
        // Only one row
        assert_eq!(db.list().unwrap().len(), 1);
    }

    #[test]
    fn update_heartbeat() {
        let db = Db::open_in_memory().unwrap();
        db.upsert(&sample_machine("m1", "prod")).unwrap();
        let ts = chrono::Utc::now().timestamp();
        db.update_heartbeat("m1", "online", ts).unwrap();
        let got = db.get("m1").unwrap().unwrap();
        assert_eq!(got.status, "online");
        assert_eq!(got.last_seen, Some(ts));
    }

    #[test]
    fn update_heartbeat_unreachable() {
        let db = Db::open_in_memory().unwrap();
        db.upsert(&sample_machine("m1", "prod")).unwrap();
        db.update_heartbeat("m1", "unreachable", 0).unwrap();
        let got = db.get("m1").unwrap().unwrap();
        assert_eq!(got.status, "unreachable");
    }

    #[test]
    fn update_capabilities() {
        let db = Db::open_in_memory().unwrap();
        db.upsert(&sample_machine("m1", "prod")).unwrap();
        let caps = Capabilities {
            version: Some("1.0.0".to_string()),
            os: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            hostname: Some("prod-host".to_string()),
            has_systemd: true,
            has_docker: true,
            has_git: false,
            has_ui_automation: false,
        };
        db.update_capabilities("m1", &caps).unwrap();
        let got = db.get("m1").unwrap().unwrap();
        let c = got.capabilities.unwrap();
        assert!(c.has_systemd);
        assert!(c.has_docker);
        assert!(!c.has_git);
        assert_eq!(c.hostname, Some("prod-host".to_string()));
    }

    #[test]
    fn capabilities_roundtrip_via_upsert() {
        let db = Db::open_in_memory().unwrap();
        let mut m = sample_machine("m1", "prod");
        m.capabilities = Some(Capabilities {
            version: None,
            os: Some("linux".to_string()),
            arch: Some("aarch64".to_string()),
            hostname: None,
            has_systemd: false,
            has_docker: false,
            has_git: true,
            has_ui_automation: false,
        });
        db.upsert(&m).unwrap();
        let got = db.get("m1").unwrap().unwrap();
        let c = got.capabilities.unwrap();
        assert!(c.has_git);
        assert!(!c.has_docker);
        assert_eq!(c.arch, Some("aarch64".to_string()));
    }
}
