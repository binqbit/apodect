"""Durable local provider state; unknown financial effects are never retried fresh."""

import json
import sqlite3
import time
from contextlib import contextmanager
from pathlib import Path

from protocol import canonical_json


class Journal:
    def __init__(self, path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with self.connect() as db:
            db.execute("PRAGMA journal_mode=WAL")
            db.execute(
                "CREATE TABLE IF NOT EXISTS records (kind TEXT, id TEXT, value TEXT NOT NULL, PRIMARY KEY(kind,id))"
            )
            db.execute("""CREATE TABLE IF NOT EXISTS effects (
                id TEXT PRIMARY KEY, request TEXT NOT NULL, state TEXT NOT NULL,
                result TEXT, attempts INTEGER NOT NULL DEFAULT 0, error TEXT,
                updated INTEGER NOT NULL)""")
        self.path.chmod(0o600)

    @contextmanager
    def connect(self):
        db = sqlite3.connect(self.path, timeout=15)
        db.row_factory = sqlite3.Row
        db.execute("PRAGMA synchronous=FULL")
        try:
            with db:
                yield db
        finally:
            db.close()

    def get(self, kind, key, default=None):
        with self.connect() as db:
            row = db.execute(
                "SELECT value FROM records WHERE kind=? AND id=?", (kind, key)
            ).fetchone()
        return json.loads(row[0]) if row else default

    def put(self, kind, key, value, immutable=False):
        encoded = canonical_json(value).decode()
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            old = db.execute(
                "SELECT value FROM records WHERE kind=? AND id=?", (kind, key)
            ).fetchone()
            if immutable and old and old[0] != encoded:
                raise ValueError(f"immutable {kind} {key} changed")
            db.execute(
                "INSERT INTO records VALUES(?,?,?) ON CONFLICT(kind,id) DO UPDATE SET value=excluded.value",
                (kind, key, encoded),
            )

    def records(self, kind):
        with self.connect() as db:
            rows = db.execute(
                "SELECT id,value FROM records WHERE kind=? ORDER BY rowid", (kind,)
            ).fetchall()
        return [(row[0], json.loads(row[1])) for row in rows]

    def resolve(self, kind, key):
        """Retain the last diagnostic while marking its successful recovery."""
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute(
                "SELECT value FROM records WHERE kind=? AND id=?", (kind, key)
            ).fetchone()
            if row is None:
                return
            value = json.loads(row[0])
            if "error" not in value or "resolved_at" in value:
                return
            value["resolved_at"] = int(time.time())
            db.execute(
                "UPDATE records SET value=? WHERE kind=? AND id=?",
                (canonical_json(value).decode(), kind, key),
            )

    def begin_effect(self, key, request):
        """Return True once, after durable intent, before any external effect."""
        encoded = canonical_json(request).decode()
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute(
                "SELECT request FROM effects WHERE id=?", (key,)
            ).fetchone()
            if row:
                if row[0] != encoded:
                    raise ValueError(
                        "financial effect ID reused with different arguments"
                    )
                return False
            db.execute(
                "INSERT INTO effects(id,request,state,updated) VALUES(?,?,'Unknown',?)",
                (key, encoded, int(time.time())),
            )
        return True

    def effect(self, key):
        with self.connect() as db:
            row = db.execute("SELECT * FROM effects WHERE id=?", (key,)).fetchone()
        if row is None:
            return None
        result = dict(row)
        for field in ("request", "result"):
            if result[field] is not None:
                result[field] = json.loads(result[field])
        return result

    def settle_effect(self, key, evidence):
        with self.connect() as db:
            changed = db.execute(
                "UPDATE effects SET state='Confirmed',result=?,error=NULL,updated=? WHERE id=?",
                (canonical_json(evidence).decode(), int(time.time()), key),
            ).rowcount
            if changed != 1:
                raise ValueError("missing effect intent")

    def note_unknown(self, key, error):
        with self.connect() as db:
            db.execute(
                """UPDATE effects SET attempts=attempts+1,error=?,updated=?,
                state=CASE WHEN attempts+1>=? THEN 'Attention' ELSE 'Unknown' END
                WHERE id=? AND state!='Confirmed'""",
                (str(error), int(time.time()), 3, key),
            )

    def unresolved(self):
        with self.connect() as db:
            rows = db.execute(
                "SELECT id FROM effects WHERE state!='Confirmed' ORDER BY updated"
            ).fetchall()
        return [self.effect(row[0]) for row in rows]
