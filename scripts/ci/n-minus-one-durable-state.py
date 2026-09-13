#!/usr/bin/env python3
"""Seed/verify representative values in every release-critical state class.

The N-1 baseline can predate a state class (v1.13.3 has no access.db and only
creates auth.db in OAuth mode) or have an older schema for it. `seed` writes a
row into every class the running baseline actually created, using only the
columns its table has, and records what it seeded. `verify` then requires
exactly those classes to survive upgrade, restart and rollback. Absent classes
are always named, and nothing is skipped silently.
"""
import argparse, json, os, sqlite3, sys, time
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument("mode", choices=("seed", "verify"))
p.add_argument("root", type=Path)
a = p.parse_args()
root = a.root
marker = "labby-n-minus-one-durable-state-v1"
# Seed current timestamps: usage.db prunes rows older than its retention
# window, and access.db keeps only the newest security events.
now = int(time.time())
manifest_path = root / "n-minus-one-seeded.json"
files = {
    "skills/n-minus-one/SKILL.md": f"---\nname: n-minus-one\ndescription: {marker}\n---\n{marker}\n",
    "artifacts/n-minus-one/probe.txt": marker + "\n",
    "snippets/n-minus-one/probe.txt": marker + "\n",
}
# database -> (table, row by column, verification query, expected result)
semantic_rows = {
    "auth.db": (
        "registered_clients",
        {"client_id": "labby-n1-client", "redirect_uris": '["http://127.0.0.1/n1"]', "created_at": now},
        "SELECT redirect_uris FROM registered_clients WHERE client_id='labby-n1-client'",
        ('["http://127.0.0.1/n1"]',),
    ),
    "access.db": (
        "access_security_events",
        {
            "event_id": "labby-n1-access-event", "occurred_at": now, "event_kind": "credential_verify",
            "decision": "deny", "reason_code": "n1_compatibility", "target_fingerprint": bytes.fromhex("11" * 32),
            "peer_fingerprint": None, "metadata_json": '{"fixture":"n1"}',
        },
        "SELECT decision,reason_code,metadata_json FROM access_security_events WHERE event_id='labby-n1-access-event'",
        ("deny", "n1_compatibility", '{"fixture":"n1"}'),
    ),
    "usage.db": (
        "upstream_calls",
        {
            "ts_unix": now, "upstream_name": "n-minus-one", "tool_name": "state-probe", "capability": "tools",
            "operation": "tool.call", "subject_scoped": 0, "actor": "release-qualification", "outcome": "success",
            "elapsed_ms": 1, "response_bytes": 1,
        },
        "SELECT outcome,actor FROM upstream_calls WHERE upstream_name='n-minus-one' AND tool_name='state-probe' ORDER BY id DESC LIMIT 1",
        ("success", "release-qualification"),
    ),
}


def seed_row(path: Path, table: str, row: dict, query: str, expected: tuple) -> str | None:
    """Seed one class; return why it is absent, or None once seeded."""
    if not path.is_file():
        return "database not created by the baseline runtime"
    with sqlite3.connect(path) as db:
        columns = {info[1] for info in db.execute(f"PRAGMA table_info({table})")}
        if not columns:
            return f"table {table} not created by the baseline runtime"
        present = [column for column in row if column in columns]
        placeholders = ",".join("?" for _ in present)
        try:
            db.execute(f"INSERT OR REPLACE INTO {table}({','.join(present)}) VALUES({placeholders})",
                       [row[column] for column in present])
            seeded = db.execute(query).fetchone()
        except sqlite3.Error as error:
            raise SystemExit(f"cannot seed {path.name}/{table} in the baseline schema: {error}")
    if seeded != expected:
        raise SystemExit(f"seeded row is not readable back from {path.name}/{table}")
    os.chmod(path, 0o600)
    return None


if a.mode == "seed":
    root.mkdir(parents=True, exist_ok=True)
    for relative, contents in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents)
        os.chmod(path, 0o600)
    seeded, absent = [], {}
    for name, (table, row, query, expected) in semantic_rows.items():
        reason = seed_row(root / name, table, row, query, expected)
        if reason is None:
            seeded.append(name)
        else:
            absent[name] = reason
    if not seeded:
        raise SystemExit("baseline runtime created none of the durable databases: " + ", ".join(semantic_rows))
    manifest_path.write_text(json.dumps({"seeded": seeded, "absent": absent}, sort_keys=True) + "\n")
    os.chmod(manifest_path, 0o600)
    for directory in (root, root / "skills", root / "artifacts", root / "snippets"):
        os.chmod(directory, 0o700)
    print("seeded durable state: " + ", ".join(seeded))
    for name, reason in absent.items():
        print(f"not in N-1, not verified: {name} ({reason})", file=sys.stderr)
else:
    if not manifest_path.is_file():
        raise SystemExit("durable-state seed manifest is missing")
    manifest = json.loads(manifest_path.read_text())
    for relative, contents in files.items():
        if (root / relative).read_text() != contents:
            raise SystemExit(f"durable-state mismatch: {relative}")
    for name in manifest["seeded"]:
        table, _, query, expected = semantic_rows[name]
        with sqlite3.connect(f"file:{root / name}?mode=ro", uri=True) as db:
            exists = db.execute("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?", (table,)).fetchone()
            row = db.execute(query).fetchone() if exists == (1,) else None
            if row != expected:
                raise SystemExit(f"semantic durable-state mismatch: {name}/{table}")
    print("verified durable state: " + ", ".join(manifest["seeded"]))
