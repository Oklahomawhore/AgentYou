"""Single-process, one-persona SQLite reference store; not a distributed scheduler."""
from __future__ import annotations
from contextlib import contextmanager
from dataclasses import asdict
from datetime import datetime, time, timedelta
import json
import sqlite3
from zoneinfo import ZoneInfo
from .types import Affect, Assessment, Event, Guards, Persona, json_text

class Store:
    def __init__(self, path: str, persona: Persona, now: float):
        self.db = sqlite3.connect(path, isolation_level=None)
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.execute("PRAGMA foreign_keys=ON")
        self.db.executescript('''
        CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS events(
          event_id TEXT PRIMARY KEY, source TEXT NOT NULL, kind TEXT NOT NULL,
          created_at REAL NOT NULL, payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS appraisals(
          event_id TEXT PRIMARY KEY REFERENCES events(event_id),
          payload TEXT NOT NULL, created_at REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS thoughts(
          event_id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS outbox(
          id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL UNIQUE,
          event_id TEXT NOT NULL, revision INTEGER NOT NULL,
          mode TEXT NOT NULL, text TEXT NOT NULL, status TEXT NOT NULL,
          created_at REAL NOT NULL, expires_at REAL NOT NULL,
          receipt_at REAL, error TEXT);
        ''')
        with self.tx():
            existing = self._get("persona")
            identity = json_text(asdict(persona))
            if existing is not None and existing != identity:
                raise ValueError("persona changed: explicit versioned migration is required")
            if existing is None:
                self._put("persona", identity)
                self._put("revision", "0")
                self._put("guards", json_text(asdict(Guards())))
                self._put("affect", json_text(asdict(Affect(updated_at=now))))
            # A previous process may have sent before crashing. Never blindly resend.
            self.db.execute("UPDATE outbox SET status='UNKNOWN', error='crash_during_delivery' WHERE status='SENDING'")
            # READY items require reconstruction and revalidation; this MVP cancels them.
            self.db.execute("UPDATE outbox SET status='CANCELLED', error='restart_revalidation_required' WHERE status='READY'")

    @contextmanager
    def tx(self):
        self.db.execute("BEGIN IMMEDIATE")
        try:
            yield
        except BaseException:
            self.db.execute("ROLLBACK")
            raise
        else:
            self.db.execute("COMMIT")

    def close(self):
        self.db.close()
    def _get(self, key: str) -> str | None:
        row = self.db.execute("SELECT value FROM meta WHERE key=?", (key,)).fetchone()
        return row[0] if row else None
    def _put(self, key: str, value: str):
        self.db.execute("INSERT INTO meta VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (key, value))
    def _bump(self):
        self._put("revision", str(self.revision() + 1))
    def revision(self) -> int:
        return int(self._get("revision") or "0")
    def affect(self) -> Affect:
        return Affect(**json.loads(self._get("affect")))
    def guards(self) -> Guards:
        data = json.loads(self._get("guards"))
        data["allowed_sources"] = tuple(data["allowed_sources"])
        return Guards(**data)
    def set_guards(self, guards: Guards):
        # Only the trusted host API may call this method. Never expose as a model tool.
        if guards.daily_limit < 0 or guards.cooldown_seconds < 0:
            raise ValueError("invalid guard limits")
        ZoneInfo(guards.timezone)
        with self.tx():
            self._put("guards", json_text(asdict(guards)))
            self._bump()

    def ingest(self, event: Event) -> bool:
        with self.tx():
            row = self.db.execute("INSERT OR IGNORE INTO events VALUES (?,?,?,?,?)",
                (event.event_id, event.source, event.kind, event.created_at, json_text(asdict(event))))
            if row.rowcount == 0:
                return False
            self._bump()
            return True

    def evidence(self, ids: tuple[str, ...]) -> list[dict] | None:
        allowed = set(self.guards().allowed_sources)
        result = []
        for id_ in ids:
            row = self.db.execute("SELECT source,payload FROM events WHERE event_id=?", (id_,)).fetchone()
            if row is None or row["source"] not in allowed:
                return None
            result.append(json.loads(row["payload"]))
        return result

    def evidence_exists(self, ids: tuple[str, ...]) -> bool:
        return self.evidence(ids) is not None

    def recent(self, limit: int = 8) -> list[dict]:
        allowed = set(self.guards().allowed_sources)
        rows = self.db.execute("SELECT payload,source FROM events ORDER BY rowid DESC LIMIT 32").fetchall()
        return [json.loads(r["payload"]) for r in rows if r["source"] in allowed][:limit]

    def notices(self) -> list[dict]:
        rows = self.db.execute("SELECT text,status,created_at FROM outbox WHERE mode='proactive' AND status IN ('SENT','UNKNOWN','SENDING') ORDER BY created_at DESC LIMIT 8").fetchall()
        return [dict(r) for r in rows]

    def apply_appraisal(self, event_id: str, assessment: Assessment, affect: Affect,
                        expected_revision: int, now: float) -> bool:
        with self.tx():
            if self.revision() != expected_revision:
                return False
            row = self.db.execute("INSERT OR IGNORE INTO appraisals VALUES (?,?,?)",
                (event_id, json_text(asdict(assessment)), now))
            if row.rowcount != 1:
                return False
            self._put("affect", json_text(asdict(affect)))
            self._bump()
            return True

    def request_reflection(self, event: Event, reason: str):
        # A request, not fabricated experience nor hidden chain-of-thought.
        payload = {"status": "pending", "evidence_ids": list(event.candidate.evidence_ids),
                   "reason": reason, "suggested_work_objective": "核实候选判断的证据，不直接发消息"}
        self.db.execute("INSERT OR IGNORE INTO thoughts VALUES (?,?,?)",
                        (event.event_id, "reflection_request", json_text(payload)))

    def budget(self, now: float, exclude: str | None = None) -> tuple[int, float | None]:
        zone = ZoneInfo(self.guards().timezone)
        day = datetime.fromtimestamp(now, zone).date()
        start = datetime.combine(day, time.min, zone).timestamp()
        end = datetime.combine(day + timedelta(days=1), time.min, zone).timestamp()
        rows = self.db.execute("SELECT id,created_at FROM outbox WHERE mode='proactive' AND status IN ('READY','SENDING','SENT','UNKNOWN')").fetchall()
        times = [r["created_at"] for r in rows if r["id"] != exclude]
        return sum(start <= t < end for t in times), max(times, default=None)

    def reserve(self, id_: str, fingerprint: str, event: Event, revision: int,
                mode: str, text: str, now: float) -> bool:
        with self.tx():
            if self.revision() != revision:
                return False
            row = self.db.execute("INSERT OR IGNORE INTO outbox VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                (id_, fingerprint, event.event_id, revision, mode, text, "READY", now, event.expires_at, None, None))
            return row.rowcount == 1

    def row(self, id_: str) -> dict:
        row = self.db.execute("SELECT * FROM outbox WHERE id=?", (id_,)).fetchone()
        if row is None:
            raise KeyError(id_)
        return dict(row)
    def rows(self) -> list[dict]:
        return [dict(r) for r in self.db.execute("SELECT * FROM outbox ORDER BY created_at")]
    def transition(self, id_: str, old: str, new: str, now: float, error: str | None = None) -> bool:
        with self.tx():
            row = self.db.execute("UPDATE outbox SET status=?,receipt_at=?,error=? WHERE id=? AND status=?",
                (new, now if new == "SENT" else None, error, id_, old))
            return row.rowcount == 1
