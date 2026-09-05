#!/usr/bin/env python3
"""Seed/verify representative values in every release-critical state class."""
import argparse, os, sqlite3
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument("mode", choices=("seed", "verify"))
p.add_argument("root", type=Path)
a = p.parse_args()
root = a.root
marker = "labby-n-minus-one-durable-state-v1"
files = {
    "skills/n-minus-one/SKILL.md": f"---\nname: n-minus-one\ndescription: {marker}\n---\n{marker}\n",
    "artifacts/n-minus-one/probe.txt": marker + "\n",
    "snippets/n-minus-one/probe.txt": marker + "\n",
}
semantic_rows = {
    "auth.db": (
        "registered_clients",
        "INSERT OR REPLACE INTO registered_clients(client_id,redirect_uris,created_at) VALUES(?,?,?)",
        ("labby-n1-client", '["http://127.0.0.1/n1"]', 1),
        "SELECT redirect_uris FROM registered_clients WHERE client_id='labby-n1-client'",
        ('["http://127.0.0.1/n1"]',),
    ),
    "access.db": (
        "access_security_events",
        "INSERT OR REPLACE INTO access_security_events(event_id,occurred_at,event_kind,decision,reason_code,target_fingerprint,peer_fingerprint,metadata_json) VALUES(?,?,?,?,?,?,NULL,?)",
        ("labby-n1-access-event", 1, "credential_verify", "deny", "n1_compatibility", bytes.fromhex("11" * 32), '{"fixture":"n1"}'),
        "SELECT decision,reason_code,metadata_json FROM access_security_events WHERE event_id='labby-n1-access-event'",
        ("deny", "n1_compatibility", '{"fixture":"n1"}'),
    ),
    "usage.db": (
        "upstream_calls",
        "INSERT INTO upstream_calls(ts_unix,upstream_name,tool_name,capability,operation,subject_scoped,actor,outcome,elapsed_ms,response_bytes) VALUES(?,?,?,?,?,?,?,?,?,?)",
        (1, "n-minus-one", "state-probe", "tools", "tool.call", 0, "release-qualification", "success", 1, 1),
        "SELECT outcome,actor FROM upstream_calls WHERE upstream_name='n-minus-one' AND tool_name='state-probe' ORDER BY id DESC LIMIT 1",
        ("success", "release-qualification"),
    ),
}

if a.mode == "seed":
    root.mkdir(parents=True, exist_ok=True)
    for relative, contents in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents)
        os.chmod(path, 0o600)
    for name, (table, statement, values, _, _) in semantic_rows.items():
        path = root / name
        if not path.is_file():
            raise SystemExit(f"runtime did not initialize required durable database: {name}")
        with sqlite3.connect(path) as db:
            exists = db.execute("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?", (table,)).fetchone()
            if exists != (1,):
                raise SystemExit(f"runtime schema missing {table} in {name}")
            db.execute(statement, values)
        os.chmod(root / name, 0o600)
    for directory in (root, root / "skills", root / "artifacts", root / "snippets"):
        os.chmod(directory, 0o700)
else:
    for relative, contents in files.items():
        if (root / relative).read_text() != contents:
            raise SystemExit(f"durable-state mismatch: {relative}")
    for name, (table, _, _, query, expected) in semantic_rows.items():
        with sqlite3.connect(f"file:{root / name}?mode=ro", uri=True) as db:
            exists = db.execute("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?", (table,)).fetchone()
            row = db.execute(query).fetchone() if exists == (1,) else None
            if row != expected:
                raise SystemExit(f"semantic durable-state mismatch: {name}/{table}")
