//! astrid-ffi — persistent state for the Android UI, all in one SQLite file
//! in app-private storage, owned by Rust. The UI never writes state itself:
//! it asks this store, and the store alone decides what is persisted.
//!
//! Tables:
//! - events            — the receipt/audit log (fed by the core session drain)
//! - chat_sessions     — one row per chat (title = first user line)
//! - chat_messages     — the transcript (role user/assistant, provider label)
//! - message_files     — uploads linked to the user message that carried them
//! - uploads           — files picked via SAF / taken by the camera (one row
//!                        per sha256: the same content is never stored twice)
//! - photos            — camera captures recorded by the bridge
//! - connectors        — per-connector state (persisted, seeded once)
//! - approval_rules    — pattern → allow/deny rules (enforcement: phase 9)
//! - tasks             — scheduler rows mirrored by Android WorkManager
//! - kv                — small config JSON blobs (provider config, active session)

use rusqlite::{params, Connection};
use uniffi::Record;

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One audit-log row.
#[derive(Debug, Clone, Record)]
pub struct EventRecord {
    pub id: u64,
    pub kind: String,
    pub text: String,
    pub created_at: u64,
}

/// One chat transcript message.
#[derive(Debug, Clone, Record)]
pub struct ChatMessageRecord {
    pub id: i64,
    pub session_id: i64,
    pub role: String,
    pub text: String,
    pub provider: String,
    pub created_at: u64,
    /// Names of files attached to this message (uploads linked at send time).
    pub files: Vec<String>,
}

/// One chat (a session of the chat surface).
#[derive(Debug, Clone, Record)]
pub struct ChatSessionRecord {
    pub id: i64,
    pub title: String,
    pub message_count: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Record)]
pub struct UploadRecord {
    pub id: i64,
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Record)]
pub struct PhotoRecord {
    pub id: i64,
    pub path: String,
    pub bytes: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Record)]
pub struct ConnectorRecord {
    pub id: String,
    pub label: String,
    pub subtitle: String,
    pub state: String,
    pub last_opened: u64,
}

#[derive(Debug, Clone, Record)]
pub struct RuleRecord {
    pub id: i64,
    pub pattern: String,
    pub decision: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Record)]
pub struct TaskRecord {
    pub id: i64,
    pub name: String,
    pub every_minutes: u32,
    pub enabled: bool,
    pub created_at: u64,
}

/// The SQLite handle. `Connection` is not `Sync`; every access goes through
/// the mutex because the UI polls from its main thread while the core drain
/// task and the WorkManager worker write from theirs.
pub struct Store {
    conn: Mutex<Connection>,
}

use std::sync::Mutex;

const CONNECTOR_SEED: &[(&str, &str, &str)] = &[
    ("whatsapp", "WhatsApp", "Messages"),
    ("email", "Email", "Inbox and drafts"),
    ("github", "GitHub", "Repos and issues"),
];

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS events (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                kind       TEXT NOT NULL,
                text       TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chat_sessions (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                title        TEXT NOT NULL DEFAULT 'New chat',
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chat_messages (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL REFERENCES chat_sessions(id),
                role       TEXT NOT NULL,
                text       TEXT NOT NULL,
                provider   TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS message_files (
                message_id INTEGER NOT NULL REFERENCES chat_messages(id),
                upload_id  INTEGER NOT NULL REFERENCES uploads(id),
                UNIQUE(message_id, upload_id)
            );
            CREATE TABLE IF NOT EXISTS uploads (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                name       TEXT NOT NULL,
                sha256     TEXT NOT NULL UNIQUE,
                bytes      INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS photos (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                path       TEXT NOT NULL,
                bytes      INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS connectors (
                id          TEXT PRIMARY KEY,
                label       TEXT NOT NULL,
                subtitle    TEXT NOT NULL,
                state       TEXT NOT NULL DEFAULT 'disconnected',
                last_opened INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS approval_rules (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern    TEXT NOT NULL,
                decision   TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                name          TEXT NOT NULL,
                every_minutes INTEGER NOT NULL,
                enabled       INTEGER NOT NULL DEFAULT 1,
                created_at    INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS kv (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        // Seed the connector rows once; state persists from here on.
        for (id, label, subtitle) in CONNECTOR_SEED {
            conn.execute(
                "INSERT OR IGNORE INTO connectors (id, label, subtitle) VALUES (?1, ?2, ?3)",
                params![id, label, subtitle],
            )?;
        }
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- event log ----------------------------------------------------

    pub fn append(&self, kind: &str, text: &str) -> rusqlite::Result<EventRecord> {
        let created = now_ms();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO events (kind, text, created_at) VALUES (?1, ?2, ?3)",
            params![kind, text, created as i64],
        )?;
        Ok(EventRecord {
            id: conn.last_insert_rowid() as u64,
            kind: kind.to_string(),
            text: text.to_string(),
            created_at: created,
        })
    }

    pub fn recent(&self, limit: u32) -> rusqlite::Result<Vec<EventRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, kind, text, created_at FROM events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(EventRecord {
                id: r.get::<_, i64>(0)? as u64,
                kind: r.get(1)?,
                text: r.get(2)?,
                created_at: r.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect()
    }

    pub fn count(&self) -> rusqlite::Result<u64> {
        let conn = self.lock();
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| {
            r.get::<_, i64>(0).map(|n| n as u64)
        })
    }

    // ---- kv ------------------------------------------------------------

    pub fn kv_get(&self, key: &str) -> rusqlite::Result<Option<String>> {
        let conn = self.lock();
        conn.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
            r.get::<_, String>(0)
        })
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
    }

    pub fn kv_del(&self, key: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute("DELETE FROM kv WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn kv_set(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- chat ----------------------------------------------------------

    pub fn insert_session(&self, title: &str) -> rusqlite::Result<ChatSessionRecord> {
        let now = now_ms();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO chat_sessions (title, created_at, updated_at) VALUES (?1, ?2, ?2)",
            params![title, now as i64],
        )?;
        let id = conn.last_insert_rowid();
        Ok(ChatSessionRecord {
            id,
            title: title.to_string(),
            message_count: 0,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn list_sessions(&self) -> rusqlite::Result<Vec<ChatSessionRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.title, s.created_at, s.updated_at,
                    (SELECT COUNT(*) FROM chat_messages m WHERE m.session_id = s.id)
             FROM chat_sessions s ORDER BY s.updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ChatSessionRecord {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: r.get::<_, i64>(2)? as u64,
                updated_at: r.get::<_, i64>(3)? as u64,
                message_count: r.get::<_, i64>(4)? as u64,
            })
        })?;
        rows.collect()
    }

    /// Insert a transcript row and bump the session's updated_at. When the
    /// session still has its placeholder title and the first user line
    /// arrives, the title becomes that line.
    pub fn insert_message(
        &self,
        session_id: i64,
        role: &str,
        text: &str,
        provider: &str,
    ) -> rusqlite::Result<ChatMessageRecord> {
        let now = now_ms();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO chat_messages (session_id, role, text, provider, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, role, text, provider, now as i64],
        )?;
        let id = conn.last_insert_rowid();
        if role == "user" {
            conn.execute(
                "UPDATE chat_sessions
                 SET title = substr(?2, 1, 48), updated_at = ?3
                 WHERE id = ?1 AND title = 'New chat'",
                params![session_id, text, now as i64],
            )?;
        } else {
            conn.execute(
                "UPDATE chat_sessions SET updated_at = ?2 WHERE id = ?1",
                params![session_id, now as i64],
            )?;
        }
        Ok(ChatMessageRecord {
            id,
            session_id,
            role: role.to_string(),
            text: text.to_string(),
            provider: provider.to_string(),
            created_at: now,
            files: Vec::new(),
        })
    }

    /// Link uploads to a message (the bubble renders them as chips).
    pub fn link_message_files(&self, message_id: i64, upload_ids: &[i64]) -> rusqlite::Result<()> {
        let conn = self.lock();
        for uid in upload_ids {
            conn.execute(
                "INSERT OR IGNORE INTO message_files (message_id, upload_id) VALUES (?1, ?2)",
                params![message_id, uid],
            )?;
        }
        Ok(())
    }

    fn file_names_for(conn: &Connection, message_id: i64) -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT u.name FROM message_files mf
             JOIN uploads u ON u.id = mf.upload_id
             WHERE mf.message_id = ?1 ORDER BY mf.rowid",
        )?;
        let rows = stmt.query_map(params![message_id], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<Vec<String>, _>>()
    }

    pub fn recent_messages(
        &self,
        session_id: i64,
        limit: u32,
    ) -> rusqlite::Result<Vec<ChatMessageRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, text, provider, created_at
             FROM chat_messages WHERE session_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![session_id, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)? as u64,
            ))
        })?;
        let mut v: Vec<ChatMessageRecord> = rows
            .collect::<Result<Vec<(i64, i64, String, String, String, u64)>, rusqlite::Error>>()?
            .into_iter()
            .map(|(id, session_id, role, text, provider, created_at)| {
                Ok(ChatMessageRecord {
                    id,
                    session_id,
                    role,
                    text,
                    provider,
                    created_at,
                    files: Self::file_names_for(&conn, id)?,
                })
            })
            .collect::<rusqlite::Result<Vec<ChatMessageRecord>>>()?;
        v.reverse(); // oldest first, transcript order
        Ok(v)
    }

    // ---- uploads & photos ----------------------------------------------

    pub fn insert_upload(&self, name: &str, sha256: &str, bytes: u64) -> rusqlite::Result<UploadRecord> {
        let conn = self.lock();
        // Same content (same sha256) is one upload: re-picking a file must
        // not create a second row or a second chip.
        let existing = conn
            .query_row(
                "SELECT id, name, sha256, bytes, created_at FROM uploads WHERE sha256 = ?1",
                params![sha256],
                |r| {
                    Ok(UploadRecord {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        sha256: r.get(2)?,
                        bytes: r.get::<_, i64>(3)? as u64,
                        created_at: r.get::<_, i64>(4)? as u64,
                    })
                },
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            });
        if let Some(rec) = existing? {
            return Ok(rec);
        }
        let now = now_ms();
        conn.execute(
            "INSERT INTO uploads (name, sha256, bytes, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, sha256, bytes as i64, now as i64],
        )?;
        Ok(UploadRecord {
            id: conn.last_insert_rowid(),
            name: name.to_string(),
            sha256: sha256.to_string(),
            bytes,
            created_at: now,
        })
    }

    pub fn list_uploads(&self) -> rusqlite::Result<Vec<UploadRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, sha256, bytes, created_at FROM uploads ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(UploadRecord {
                id: r.get(0)?,
                name: r.get(1)?,
                sha256: r.get(2)?,
                bytes: r.get::<_, i64>(3)? as u64,
                created_at: r.get::<_, i64>(4)? as u64,
            })
        })?;
        rows.collect()
    }

    pub fn insert_photo(&self, path: &str, bytes: u64) -> rusqlite::Result<PhotoRecord> {
        let now = now_ms();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO photos (path, bytes, created_at) VALUES (?1, ?2, ?3)",
            params![path, bytes as i64, now as i64],
        )?;
        Ok(PhotoRecord {
            id: conn.last_insert_rowid(),
            path: path.to_string(),
            bytes,
            created_at: now,
        })
    }

    pub fn list_photos(&self) -> rusqlite::Result<Vec<PhotoRecord>> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT id, path, bytes, created_at FROM photos ORDER BY id DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(PhotoRecord {
                id: r.get(0)?,
                path: r.get(1)?,
                bytes: r.get::<_, i64>(2)? as u64,
                created_at: r.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect()
    }

    // ---- connectors ------------------------------------------------------

    pub fn list_connectors(&self) -> rusqlite::Result<Vec<ConnectorRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, label, subtitle, state, last_opened FROM connectors ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ConnectorRecord {
                id: r.get(0)?,
                label: r.get(1)?,
                subtitle: r.get(2)?,
                state: r.get(3)?,
                last_opened: r.get::<_, i64>(4)? as u64,
            })
        })?;
        rows.collect()
    }

    pub fn set_connector(&self, id: &str, state: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE connectors SET state = ?2, last_opened = ?3 WHERE id = ?1",
            params![id, state, now_ms() as i64],
        )?;
        Ok(())
    }

    // ---- approval rules ---------------------------------------------------

    pub fn add_rule(&self, pattern: &str, decision: &str) -> rusqlite::Result<RuleRecord> {
        let now = now_ms();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO approval_rules (pattern, decision, created_at) VALUES (?1, ?2, ?3)",
            params![pattern, decision, now as i64],
        )?;
        Ok(RuleRecord {
            id: conn.last_insert_rowid(),
            pattern: pattern.to_string(),
            decision: decision.to_string(),
            created_at: now,
        })
    }

    pub fn delete_rule(&self, id: i64) -> rusqlite::Result<bool> {
        let n = self
            .lock()
            .execute("DELETE FROM approval_rules WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn list_rules(&self) -> rusqlite::Result<Vec<RuleRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, pattern, decision, created_at FROM approval_rules ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(RuleRecord {
                id: r.get(0)?,
                pattern: r.get(1)?,
                decision: r.get(2)?,
                created_at: r.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect()
    }

    // ---- scheduler tasks ---------------------------------------------------

    pub fn add_task(&self, name: &str, every_minutes: u32) -> rusqlite::Result<TaskRecord> {
        let now = now_ms();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO tasks (name, every_minutes, enabled, created_at) VALUES (?1, ?2, 1, ?3)",
            params![name, every_minutes as i64, now as i64],
        )?;
        Ok(TaskRecord {
            id: conn.last_insert_rowid(),
            name: name.to_string(),
            every_minutes,
            enabled: true,
            created_at: now,
        })
    }

    pub fn set_task_enabled(&self, id: i64, enabled: bool) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE tasks SET enabled = ?2 WHERE id = ?1",
            params![id, enabled as i64],
        )?;
        Ok(())
    }

    pub fn list_tasks(&self) -> rusqlite::Result<Vec<TaskRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, every_minutes, enabled, created_at FROM tasks ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TaskRecord {
                id: r.get(0)?,
                name: r.get(1)?,
                every_minutes: r.get::<_, i64>(2)? as u32,
                enabled: r.get::<_, i64>(3)? != 0,
                created_at: r.get::<_, i64>(4)? as u64,
            })
        })?;
        rows.collect()
    }
}
