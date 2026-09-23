use crate::{model::*, Error, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

/// One process and one actor own this store. The advisory lock is held until Drop.
pub struct Store {
    db: Connection,
    _lock: File,
}
impl Store {
    pub fn open(path: impl AsRef<Path>, persona: Persona, now: i64) -> Result<Self> {
        let path = path.as_ref();
        // Canonicalize the parent so relative aliases share the same lock path.
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let path = if path.exists() {
            path.canonicalize()?
        } else {
            parent.canonicalize()?.join(
                path.file_name()
                    .ok_or_else(|| Error::Invalid("database filename".into()))?,
            )
        };
        let mut lock_name = path.as_os_str().to_os_string();
        lock_name.push(".lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_name)?;
        lock.try_lock_exclusive()
            .map_err(|_| Error::Invalid("database already owned by another runtime".into()))?;
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS inbox(id TEXT PRIMARY KEY, payload TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('pending','processing','done')));
            CREATE TABLE IF NOT EXISTS decisions(event_id TEXT PRIMARY KEY REFERENCES inbox(id),
                payload TEXT NOT NULL, appraisal TEXT, candidate_id TEXT, kind TEXT,
                action TEXT NOT NULL, created_at_ms INTEGER NOT NULL);
            CREATE UNIQUE INDEX IF NOT EXISTS speak_candidate ON decisions(candidate_id) WHERE action='speak';")?;
        let mut store = Self { db, _lock: lock };
        let identity = serde_json::to_string(&persona)?;
        let tx = store.db.transaction()?;
        let existing: Option<String> = tx
            .query_row("SELECT value FROM meta WHERE key='persona'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(existing) = existing {
            if existing != identity {
                return Err(Error::Invalid(
                    "persona changed; explicit migration required".into(),
                ));
            }
            let schema: String =
                tx.query_row("SELECT value FROM meta WHERE key='schema'", [], |r| {
                    r.get(0)
                })?;
            if schema != "1" {
                return Err(Error::Invalid("unsupported schema version".into()));
            }
        } else {
            for (key, value) in [
                ("persona", identity),
                ("revision", "0".into()),
                ("schema", "1".into()),
                ("guards", serde_json::to_string(&Guards::default())?),
                ("affect", serde_json::to_string(&Affect::initial(now))?),
            ] {
                tx.execute("INSERT INTO meta VALUES (?,?)", params![key, value])?;
            }
        }
        // No external effects exist in shadow mode. Re-evaluate interrupted reads only.
        tx.execute(
            "UPDATE inbox SET status='pending' WHERE status='processing'",
            [],
        )?;
        tx.commit()?;
        Ok(store)
    }
    fn meta<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<T> {
        let value: String =
            self.db
                .query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))?;
        Ok(serde_json::from_str(&value)?)
    }
    pub fn guards(&self) -> Result<Guards> {
        self.meta("guards")
    }
    pub fn affect(&self) -> Result<Affect> {
        self.meta("affect")
    }
    pub fn revision(&self) -> Result<u64> {
        self.meta("revision")
    }
    pub fn set_guards(&mut self, guards: &Guards) -> Result<()> {
        guards.validate()?;
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE meta SET value=? WHERE key='guards'",
            [serde_json::to_string(guards)?],
        )?;
        tx.execute(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='revision'",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn ingest(&mut self, event: &Event) -> Result<bool> {
        event.validate()?;
        if !self.guards()?.allowed_sources.contains(&event.source) {
            return Err(Error::Invalid("source_not_authorized".into()));
        }
        let tx = self.db.transaction()?;
        let added = tx.execute(
            "INSERT OR IGNORE INTO inbox VALUES (?,?,'pending')",
            params![event.id, serde_json::to_string(event)?],
        )? == 1;
        if added {
            tx.execute(
                "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='revision'",
                [],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }
    pub fn pending(&self) -> Result<Vec<Event>> {
        let mut stmt = self
            .db
            .prepare("SELECT payload FROM inbox WHERE status='pending' ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn mark_processing(&self, id: &str) -> Result<()> {
        self.db.execute(
            "UPDATE inbox SET status='processing' WHERE id=? AND status='pending'",
            [id],
        )?;
        Ok(())
    }
    pub fn snapshot(&self, event: &Event, now: i64) -> Result<Snapshot> {
        let guards = self.guards()?;
        let mut evidence = Vec::new();
        if let Some(c) = &event.candidate {
            for id in &c.evidence_ids {
                let raw: Option<String> = self
                    .db
                    .query_row("SELECT payload FROM inbox WHERE id=?", [id], |r| r.get(0))
                    .optional()?;
                let e: Event = serde_json::from_str(
                    &raw.ok_or_else(|| Error::Invalid("unknown_evidence".into()))?,
                )?;
                if !guards.allowed_sources.contains(&e.source)
                    || now >= e.expires_at_ms
                    || now < e.created_at_ms.saturating_sub(5000)
                {
                    return Err(Error::Invalid("evidence_expired_or_unauthorized".into()));
                }
                evidence.push(e);
            }
        }
        Ok(Snapshot {
            revision: self.revision()?,
            persona: self.meta("persona")?,
            affect: self.affect()?,
            event: event.clone(),
            evidence,
        })
    }
    /// Phase A uses a documented rolling 24-hour window, not local calendar days.
    pub fn budget(&self, kind: InitiativeKind, now: i64) -> Result<(u32, Option<i64>)> {
        Ok(self.db.query_row("SELECT COALESCE(SUM(created_at_ms > ?),0), MAX(created_at_ms) FROM decisions WHERE kind=? AND action='speak'",
            params![now.saturating_sub(86_400_000), serde_json::to_string(&kind)?], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }
    pub fn candidate_used(&self, id: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM decisions WHERE candidate_id=? AND action='speak')",
            [id],
            |r| r.get(0),
        )?)
    }
    pub fn finish(
        &mut self,
        event: &Event,
        decision: &Decision,
        appraisal: Option<&Appraisal>,
        affect: Option<Affect>,
        now: i64,
    ) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO decisions VALUES (?,?,?,?,?,?,?)",
            params![
                event.id,
                serde_json::to_string(decision)?,
                appraisal.map(serde_json::to_string).transpose()?,
                event.candidate.as_ref().map(|c| &c.id),
                event
                    .candidate
                    .as_ref()
                    .map(|c| serde_json::to_string(&c.kind))
                    .transpose()?,
                serde_json::to_value(decision.action)?.as_str().unwrap(),
                now
            ],
        )?;
        if let Some(affect) = affect {
            tx.execute(
                "UPDATE meta SET value=? WHERE key='affect'",
                [serde_json::to_string(&affect)?],
            )?;
            tx.execute(
                "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='revision'",
                [],
            )?;
        }
        tx.execute("UPDATE inbox SET status='done' WHERE id=?", [&event.id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn forget_events(
        &mut self,
        ids: &[String],
        now: i64,
    ) -> Result<std::collections::HashSet<String>> {
        let events: Vec<Event> = {
            let mut stmt = self.db.prepare("SELECT payload FROM inbox")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|r| Ok(serde_json::from_str(&r?)?))
                .collect::<Result<_>>()?
        };
        let mut removed: std::collections::HashSet<String> = ids.iter().cloned().collect();
        loop {
            let before = removed.len();
            for event in &events {
                if event
                    .candidate
                    .as_ref()
                    .is_some_and(|c| c.evidence_ids.iter().any(|id| removed.contains(id)))
                {
                    removed.insert(event.id.clone());
                }
            }
            if before == removed.len() {
                break;
            }
        }
        let tx = self.db.transaction()?;
        for id in &removed {
            tx.execute("DELETE FROM decisions WHERE event_id=?", [id])?;
            tx.execute("DELETE FROM inbox WHERE id=?", [id])?;
        }
        tx.execute(
            "UPDATE meta SET value=? WHERE key='affect'",
            [serde_json::to_string(&Affect::initial(now))?],
        )?;
        tx.execute(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='revision'",
            [],
        )?;
        tx.commit()?;
        Ok(removed)
    }
    pub fn decisions(&self) -> Result<Vec<Decision>> {
        let mut stmt = self
            .db
            .prepare("SELECT payload FROM decisions ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
}
