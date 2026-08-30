#!/usr/bin/env python3
"""Exercise a WAL-safe OAuth backup and isolated restore with all key material."""

import sqlite3
import tempfile
from pathlib import Path


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="labby-auth-backup-") as raw:
        root = Path(raw)
        live, backup, restore = root / "live", root / "backup", root / "restore"
        live.mkdir(); backup.mkdir(); restore.mkdir()
        database = live / "auth.db"
        signing_key = live / "auth-jwt.pem"
        provider_key = live / "token-encryption.key"
        signing_key.write_bytes(b"fixture-signing-key")
        provider_key.write_bytes(b"fixture-provider-encryption-key")
        with sqlite3.connect(database) as connection:
            connection.execute("PRAGMA journal_mode=WAL")
            connection.execute("CREATE TABLE grants (id TEXT PRIMARY KEY, subject TEXT)")
            connection.execute("INSERT INTO grants VALUES ('grant-1', 'subject-1')")
            connection.commit()
            with sqlite3.connect(backup / "auth.db") as destination:
                connection.backup(destination)
        (backup / signing_key.name).write_bytes(signing_key.read_bytes())
        (backup / provider_key.name).write_bytes(provider_key.read_bytes())
        for artifact in backup.iterdir():
            (restore / artifact.name).write_bytes(artifact.read_bytes())
        with sqlite3.connect(restore / "auth.db") as connection:
            assert connection.execute("PRAGMA integrity_check").fetchone() == ("ok",)
            assert connection.execute("SELECT * FROM grants").fetchall() == [("grant-1", "subject-1")]
        assert (restore / signing_key.name).read_bytes() == signing_key.read_bytes()
        assert (restore / provider_key.name).read_bytes() == provider_key.read_bytes()
    print("auth backup restore drill passed")


if __name__ == "__main__":
    main()
