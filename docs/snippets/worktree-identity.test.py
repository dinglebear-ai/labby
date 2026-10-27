#!/usr/bin/env python3
"""Disposable Git/SQLite fixtures: no network or existing task state changes."""
from typing import Any

import importlib.util
import io
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest.mock import patch

spec=importlib.util.spec_from_file_location("identity",Path(__file__).with_name("worktree-identity.py"))
identity=importlib.util.module_from_spec(spec)
spec.loader.exec_module(identity)

def git(root: Path, *args: str) -> str:
    return subprocess.run(["git","-C",str(root),*args],check=True,capture_output=True,text=True).stdout.strip()

class IdentityTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp=tempfile.TemporaryDirectory(prefix="labby-identity-",dir="/tmp")
        self.home=Path(self.tmp.name).resolve()
        self.repo=self.home/"repo"
        self.repo.mkdir()
        git(self.repo,"init")
        git(self.repo,"config","user.email","fixture@example.invalid")
        git(self.repo,"config","user.name","Fixture")
        (self.repo/".gitignore").write_text("worktrees/\n")
        (self.repo/"file").write_text("fixture")
        git(self.repo,"add",".")
        git(self.repo,"commit","-m","fixture")
        git(self.repo,"remote","add","origin","git@github.com:example/repo.git")
        (self.repo/"worktrees").mkdir()
        self.wt=self.repo/"worktrees"/"owned ' $(never-execute)"
        self.assertEqual(git(self.repo,"check-ignore",str(self.wt)),str(self.wt))
        git(self.repo,"worktree","add","-b","feature/LAB-123",str(self.wt))
        self.head=git(self.wt,"rev-parse","HEAD")
        self.db=self.home/".codex/state_5.sqlite"
        self.db.parent.mkdir()
        self.c=sqlite3.connect(self.db)
        self.c.executescript("CREATE TABLE threads(id TEXT,cwd TEXT,git_sha TEXT,git_branch TEXT,git_origin_url TEXT,archived INTEGER,title TEXT); CREATE TABLE thread_attachments(id TEXT,thread_id TEXT,attachment_type TEXT,payload TEXT);")
        self.sid="11111111-1111-4111-8111-111111111111"
        self.c.execute("INSERT INTO threads VALUES(?,?,?,?,?,?,?)",(self.sid,str(self.wt),self.head,"feature/LAB-123","https://token@github.com/example/repo.git",1,"LAB-123 https://github.com/example/repo/pull/7 private words must not escape"))
        self.c.commit()
        self.reader=identity.load_identity_reader()
    def tearDown(self) -> None:
        self.c.close()
        self.tmp.cleanup()  # owned fixture only, including its disposable .git
    def run_identity(self, **kwargs: Any) -> dict[str, Any]:
        return identity.inspect({"path":str(self.wt),**kwargs},home=self.home,hostname="example-host",reader=self.reader)
    def test_reader_is_repository_owned(self) -> None:
        self.assertEqual(identity.HELPER.parent, Path(identity.__file__).parent)
        self.assertTrue(identity.HELPER.is_file())

    def test_git_capture_overflow_terminates_and_reaps_writer(self) -> None:
        fake = self.home / 'bin'
        fake.mkdir()
        # An unrelated PATH interpreter must never run this owned writer.
        wrong_interpreter = fake / 'python3'
        wrong_interpreter.write_text('#!/bin/sh\nexit 92\n')
        wrong_interpreter.chmod(0o700)
        pid_file = self.home / 'writer.pid'
        program = fake / 'git'
        # Run the fixture with this test's interpreter, avoiding PATH shims or
        # toolchain activation consuming the product's unchanged 5s deadline.
        program.write_text(f'#!{sys.executable}\nimport os,sys,time\n'
                           'open(os.environ["WRITER_PID"],"w").write(str(os.getpid()))\n'
                           'streams = [sys.stdout.buffer, sys.stderr.buffer] if os.environ["WRITER_STREAM"] == "both" else [sys.stdout.buffer if os.environ["WRITER_STREAM"] == "stdout" else sys.stderr.buffer]\n'
                           'for stream in streams: stream.write(b"x" * ((192 if len(streams) == 2 else 512) * 1024)); stream.flush()\n'
                           'time.sleep(30)\n')
        program.chmod(0o700)
        for stream in ('stdout', 'stderr', 'both'):
            with self.subTest(stream=stream), patch.dict(os.environ, PATH=str(fake)+os.pathsep+os.environ['PATH'], WRITER_PID=str(pid_file), WRITER_STREAM=stream):
                pid_file.unlink(missing_ok=True)
                with self.assertRaises(self.reader.ContextError) as raised:
                    self.reader.git(self.wt, 'status')
                self.assertEqual(raised.exception.kind, 'git_output_budget', str(raised.exception))
                pid = int(pid_file.read_text())
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)

    def test_status_stream_preserves_rename_and_bounded_projection(self) -> None:
        git(self.wt, 'mv', 'file', 'renamed')
        for number in range(40):
            (self.wt / f'untracked-{number:02}').write_text('fixture')
        _, info = self.reader.repository_info(self.wt, None)
        self.assertEqual(info['changes_count'], 41)
        self.assertEqual(len(info['changes']), 30)
        self.assertTrue(info['changes_truncated'])
        self.assertEqual(info['changes'][0], {'status':'R ', 'path':'renamed', 'original_path':'file'})
        raw = subprocess.run(['git', '-C', str(self.wt), 'status', '--porcelain=v1', '-z', '--untracked-files=normal'], check=True, capture_output=True).stdout
        self.assertEqual(info['status_sha256'], self.reader.digest(raw))

    def test_symlink_dirty_many_sessions_and_redaction(self) -> None:
        alias=self.home/"alias"
        alias.symlink_to(self.wt,target_is_directory=True)
        (self.wt/"file").write_text("dirty")
        sid2="22222222-2222-4222-8222-222222222222"
        self.c.execute("INSERT INTO threads VALUES(?,?,?,?,?,?,?)",(sid2,str(self.home),self.head,"feature/LAB-123","ssh://git@github.com/example/repo.git",0,""))
        self.c.execute("INSERT INTO thread_attachments VALUES(?,?,?,?)",("a",sid2,"worktree",json.dumps({"root":str(self.wt)})))
        self.c.commit()
        out=self.run_identity(path=str(alias))
        self.assertEqual(out["identity"]["root"],str(self.wt))
        self.assertTrue(out["identity"]["dirty"])
        self.assertEqual(out["identity"]["common_dir"],str(self.repo/".git"))
        self.assertEqual(len(out["sessions"]["verified"]),2)
        self.assertTrue(out["sessions"]["verified"][0]["archived"])
        self.assertEqual(out["hints"]["pull_requests"][0]["number"],7)
        serialized=json.dumps(out)
        self.assertNotIn("token@",serialized)
        self.assertNotIn("private words",serialized)
        self.assertNotIn("transcript",serialized.replace("transcripts_read",""))
    def test_detached_and_multiple_remotes(self) -> None:
        git(self.wt,"checkout","--detach")
        git(self.repo,"remote","add","fork","https://github.com/other/repo.git")
        out=self.run_identity()
        self.assertTrue(out["identity"]["detached"])
        self.assertIsNone(out["identity"]["branch"])
        self.assertTrue(out["identity"]["multiple_remotes"])
        self.assertEqual(len(out["identity"]["repository_candidates"]),2)
    def test_remote_ports_and_credential_redaction(self) -> None:
        git(self.repo,"remote","set-url","origin","ssh://git@github.com:22/example/repo.git")
        out=self.run_identity()
        self.assertEqual(out["identity"]["repository_candidates"],["example/repo"])
        self.assertEqual(self.reader.remote_info("https://user:secret@GitHub.com:443/example/repo.git"),{"url":"https://github.com/example/repo","github_repo":"example/repo"})
    def test_metadata_open_failure_preserves_git_evidence(self) -> None:
        from unittest.mock import patch
        with patch.object(identity.sqlite3,"connect",side_effect=sqlite3.OperationalError("denied")):
            out=self.run_identity()
        self.assertTrue(out["ok"])
        self.assertEqual(out["identity"]["head"],self.head)
        self.assertEqual(out["sessions"]["coverage"]["reason"],"metadata_read_failed")
        class LegacyConnection:
            closed = False
            def close(connection): connection.closed = True
        legacy = LegacyConnection()
        with patch.object(identity.sqlite3, "connect", return_value=legacy):
            out = self.run_identity()
        self.assertEqual(out["identity"]["head"], self.head)
        self.assertEqual(out["sessions"]["coverage"]["reason"], "metadata_byte_limit_unavailable")
        self.assertTrue(legacy.closed)
    def test_git_only_is_candidate_not_path_ownership(self) -> None:
        self.c.execute("UPDATE threads SET cwd=?",(str(self.home/"old-deleted-name"),))
        self.c.commit()
        out=self.run_identity()
        self.assertEqual(out["sessions"]["verified"],[])
        self.assertEqual(out["sessions"]["candidates"][0]["association"],"git_metadata_only")
        self.assertEqual(out["hints"]["pull_requests"],[])
    def test_deleted_registered_path(self) -> None:
        moved=self.home/"renamed-path"
        self.wt.rename(moved)  # fixture models external rename; never repairs Git
        out=self.run_identity(repo_hint=str(self.repo))
        self.assertEqual(out["identity"]["path_state"],"missing_registered")
        self.assertIsNone(out["identity"]["dirty"])
        self.assertEqual(out["identity"]["git_identity_source"],"git_worktree_registration")
        self.assertTrue(out["identity"]["registration"]["prunable"])
        with self.assertRaises(self.reader.ContextError):
            self.run_identity(path=str(self.home/"unregistered"),repo_hint=str(self.repo))
    def test_pagination_and_archived_attachment(self) -> None:
        sid2="22222222-2222-4222-8222-222222222222"
        self.c.execute("INSERT INTO threads VALUES(?,?,?,?,?,?,?)",(sid2,str(self.home),None,None,None,1,""))
        payload={"worktree":{"root":str(self.wt)},"pullRequests":[{"url":"https://github.com/example/repo/pull/9"}]}
        self.c.execute("INSERT INTO thread_attachments VALUES(?,?,?,?)",("a",sid2,"archived_worktree",json.dumps(payload)))
        self.c.commit()
        first=self.run_identity(session_limit=1)
        self.assertTrue(first["sessions"]["coverage"]["incomplete"])
        second=self.run_identity(session_limit=1,session_cursor=first["sessions"]["coverage"]["next_cursor"])
        self.assertEqual(second["sessions"]["verified"][0]["id"],sid2)
        self.assertEqual(second["hints"]["pull_requests"][0]["number"],9)
    def test_missing_metadata_and_wrong_host(self) -> None:
        self.c.close()
        self.db.unlink()
        out=self.run_identity()
        self.assertFalse(out["sessions"]["coverage"]["available"])
        with self.assertRaises(self.reader.ContextError):
            identity.inspect({"path":str(self.wt),"expected_host":"example-host"},home=self.home,hostname="another-host",reader=self.reader)
        self.c=sqlite3.connect(self.db)
    def test_parent_repository_excludes_nested_worktree_session(self) -> None:
        out = self.run_identity(path=str(self.repo))
        self.assertEqual(out["sessions"]["verified"], [])
        self.assertEqual(out["sessions"]["candidates"][0]["id"], self.sid)
        self.assertEqual(out["hints"]["pull_requests"], [])
        self.assertEqual(out["hints"]["issue_keys"], [])

    def test_malformed_archived_attachment_preserves_identity(self) -> None:
        for value in (None, {}, "bad"):
            with self.subTest(value=value):
                self.c.execute("DELETE FROM thread_attachments")
                self.c.execute("INSERT INTO thread_attachments VALUES(?,?,?,?)", (
                    "bad", self.sid, "archived_worktree",
                    json.dumps({"worktree": {"root": str(self.wt)}, "pullRequests": value})))
                self.c.commit()
                out = self.run_identity()
                self.assertTrue(out["ok"])
                self.assertTrue(out["sessions"]["coverage"]["malformed_metadata"])
                self.assertEqual(out["identity"]["head"], self.head)
        # Small byte size does not imply shallow JSON. Metadata parser failures
        # must retain the separately established Git identity too.
        self.c.execute("DELETE FROM thread_attachments")
        nested = '{"worktree":{"root":' + json.dumps(str(self.wt)) + '},"pullRequests":' + '[' * 1200 + '0' + ']' * 1200 + '}'
        self.c.execute("INSERT INTO thread_attachments VALUES(?,?,?,?)", ("deep", self.sid, "archived_worktree", nested))
        self.c.commit()
        out = self.run_identity()
        self.assertTrue(out["sessions"]["coverage"]["malformed_metadata"])
        self.assertEqual(out["identity"]["head"], self.head)

    def test_nullable_metadata_and_invalid_cursor_types(self) -> None:
        self.c.execute("UPDATE threads SET cwd=NULL,git_origin_url=NULL,git_branch=NULL")
        self.c.commit()
        self.assertEqual(self.run_identity()["sessions"]["verified"], [])
        for cursor in (None, 0, False, [], {}, "-" * 36, "a" * 36):
            with self.subTest(cursor=cursor):
                with self.assertRaises(self.reader.ContextError):
                    self.run_identity(session_cursor=cursor)

    def test_metadata_schema_failure_and_input_validation(self) -> None:
        self.c.execute("DROP TABLE threads")
        self.c.commit()
        self.assertFalse(self.run_identity()["sessions"]["coverage"]["available"])
        for values in ({"session_limit":0},{"session_cursor":"bad"},{"path":"relative"},{"extra":"command"}):
            with self.assertRaises(self.reader.ContextError):self.run_identity(**values)

    def test_sqlite_byte_limit_precedes_queries(self) -> None:
        connect = sqlite3.connect
        seen = []
        class AdmissionConnection(sqlite3.Connection):
            def execute(connection, sql, parameters=()):
                seen.append(connection.getlimit(sqlite3.SQLITE_LIMIT_LENGTH))
                return super().execute(sql, parameters)
        with patch.object(identity.sqlite3, "connect", side_effect=lambda *a, **kw: connect(*a, **kw, factory=AdmissionConnection)):
            out = self.run_identity()
        self.assertTrue(out["sessions"]["coverage"]["available"])
        self.assertTrue(seen)
        self.assertTrue(all(value <= 65536 for value in seen), seen)

    def test_attachment_admission_precedes_python_rows_and_json(self) -> None:
        connect = sqlite3.connect
        observed = []
        class ProjectionCursor(sqlite3.Cursor):
            def fetchall(cursor):
                rows = super().fetchall()
                for row in rows:
                    for value in row:
                        if isinstance(value, str):
                            observed.append(len(value.encode("utf-8")))
                return rows
        class ProjectionConnection(sqlite3.Connection):
            def execute(connection, sql, parameters=()):
                return connection.cursor(factory=ProjectionCursor).execute(sql, parameters)
        loads = json.loads
        parsed_sizes = []
        def bounded_loads(value, *args, **kwargs):
            if isinstance(value, str): parsed_sizes.append(len(value.encode("utf-8")))
            return loads(value, *args, **kwargs)
        for padding in ("x" * 17000, "é" * 9000):
            with self.subTest(bytes=len(padding.encode("utf-8"))):
                payload = json.dumps({"root": str(self.wt), "url": "https://github.com/example/repo/pull/9", "padding": padding}, ensure_ascii=False)
                self.c.execute("DELETE FROM thread_attachments")
                self.c.execute("INSERT INTO thread_attachments VALUES(?,?,?,?)", ("a", self.sid, "pull_request", payload))
                self.c.commit()
                observed.clear(); parsed_sizes.clear()
                with patch.object(identity.sqlite3, "connect", side_effect=lambda *a, **kw: connect(*a, **kw, factory=ProjectionConnection)), patch.object(identity.json, "loads", side_effect=bounded_loads):
                    out = self.run_identity()
                self.assertTrue(out["ok"])
                self.assertEqual(out["identity"]["head"], self.head)
                self.assertTrue(out["sessions"]["coverage"]["malformed_metadata"])
                self.assertTrue(all(size <= 16384 for size in observed), observed)
                self.assertTrue(all(size <= 16000 for size in parsed_sizes), parsed_sizes)
                self.assertNotIn(9, [p["number"] for p in out["hints"]["pull_requests"]])
        self.c.execute("DELETE FROM thread_attachments")
        self.c.commit()
        originals = {"cwd": str(self.wt), "git_branch": "feature/LAB-123", "git_origin_url": "https://token@github.com/example/repo.git"}
        for field in ("cwd", "git_branch", "git_origin_url"):
            with self.subTest(thread_field=field):
                self.c.execute(f"UPDATE threads SET {field}=?", ("é" * 9000,))
                self.c.commit()
                observed.clear()
                with patch.object(identity.sqlite3, "connect", side_effect=lambda *a, **kw: connect(*a, **kw, factory=ProjectionConnection)):
                    out = self.run_identity()
                self.assertTrue(out["sessions"]["coverage"]["malformed_metadata"])
                self.assertTrue(all(size <= 16384 for size in observed), observed)
                self.assertEqual(out["identity"]["head"], self.head)
                self.c.execute(f"UPDATE threads SET {field}=?", (originals[field],))
                self.c.commit()

    def test_oversized_root_attachment_is_not_json_parsed(self) -> None:
        connect = sqlite3.connect
        loads = json.loads
        seen = []
        def json_valid(value):
            if value is None: return 0
            seen.append(len(value.encode("utf-8")))
            try: loads(value); return 1
            except ValueError: return 0
        def json_extract(value, field):
            if value is None: return None
            seen.append(len(value.encode("utf-8")))
            obj = loads(value)
            return obj.get("root") if field == "$.root" else None
        def monitored_connect(*args, **kwargs):
            connection = connect(*args, **kwargs)
            connection.create_function("json_valid", 1, json_valid)
            connection.create_function("json_extract", 2, json_extract)
            return connection
        self.c.execute("UPDATE threads SET cwd=?,git_sha=NULL,git_branch=NULL,git_origin_url=NULL", (str(self.home / "elsewhere"),))
        for padding in ("x" * 17000, "é" * 9000):
            with self.subTest(bytes=len(padding.encode("utf-8"))):
                self.c.execute("DELETE FROM thread_attachments")
                self.c.execute("INSERT INTO thread_attachments VALUES(?,?,?,?)", ("a", self.sid, "worktree", json.dumps({"root": str(self.wt), "padding": padding}, ensure_ascii=False)))
                self.c.commit(); seen.clear()
                with patch.object(identity.sqlite3, "connect", side_effect=monitored_connect):
                    out = self.run_identity()
                self.assertTrue(out["sessions"]["coverage"]["malformed_metadata"])
                self.assertEqual(out["sessions"]["verified"], [])
                self.assertTrue(all(size <= 16000 for size in seen), seen)
                self.assertEqual(out["identity"]["head"], self.head)

    def test_large_metadata_records_preserve_git_identity(self) -> None:
        for field in ("payload", "cwd", "git_sha", "git_branch", "git_origin_url", "title"):
            for text in ("x" * 1048576, "é" * 524288):
                with self.subTest(field=field, multibyte=text.startswith("é")):
                    if field == "payload":
                        self.c.execute("INSERT INTO thread_attachments VALUES(?,?,?,?)", ("a", self.sid, "worktree", text))
                    else:
                        self.c.execute(f"UPDATE threads SET {field}=?", (text,))
                    self.c.commit()
                    out = self.run_identity()
                    self.assertTrue(out["ok"])
                    self.assertEqual(out["identity"]["head"], self.head)
                    self.assertFalse(out["sessions"]["coverage"]["available"])
                    if field == "payload":
                        cli = subprocess.run([sys.executable, str(identity.__file__), "--input-json", json.dumps({"path": str(self.wt)})], env={**os.environ, "HOME": str(self.home)}, check=True, capture_output=True, text=True, timeout=10)
                        self.assertLessEqual(len(cli.stdout.encode("utf-8")), 14001)
                        report = json.loads(cli.stdout)
                        self.assertEqual(report["identity"]["head"], self.head)
                        self.assertFalse(report["sessions"]["coverage"]["available"])
                    self.c.execute("DELETE FROM thread_attachments")
                    self.c.execute("DELETE FROM threads")
                    self.c.execute("INSERT INTO threads VALUES(?,?,?,?,?,?,?)", (self.sid, str(self.wt), self.head, "feature/LAB-123", "https://github.com/example/repo.git", 1, ""))
                    self.c.commit()

    def test_attachment_overflow_is_separate_from_thread_cursor(self) -> None:
        self.c.execute("UPDATE threads SET title=''")
        for count in (32, 33):
            with self.subTest(count=count):
                self.c.execute("DELETE FROM thread_attachments")
                for number in range(count):
                    payload = {"root": str(self.wt)}
                    if number == 32: payload["url"] = "https://github.com/example/repo/pull/99"
                    self.c.execute("INSERT INTO thread_attachments VALUES(?,?,?,?)", (f"{number:03}", self.sid, "pull_request", json.dumps(payload)))
                self.c.commit()
                out = self.run_identity()
                coverage = out["sessions"]["coverage"]
                self.assertEqual(coverage["attachment_inspection_incomplete"], count > 32)
                self.assertIsNone(coverage["next_cursor"])
                self.assertFalse(coverage["incomplete"])
                self.assertEqual(out["hints"]["pull_requests"], [])

    def test_malformed_scan_boundaries_never_publish_unusable_cursors(self) -> None:
        for bad_id in (lambda n: f"bad-row-{n:03}", lambda n: sqlite3.Binary(f"bad-row-{n:03}".encode())):
            with self.subTest(storage=type(bad_id(0)).__name__):
                self.c.execute("DELETE FROM threads")
                for number in range(201):
                    self.c.execute("INSERT INTO threads VALUES(?,?,?,?,?,?,?)", (bad_id(number), str(self.wt), None, None, None, 0, ""))
                self.c.execute("INSERT INTO threads VALUES(?,?,?,?,?,?,?)", ("ffffffff-ffff-4fff-8fff-ffffffffffff", str(self.wt), None, None, None, 0, ""))
                self.c.commit()
                out = self.run_identity()
                coverage = out["sessions"]["coverage"]
                self.assertTrue(coverage["malformed_metadata"])
                self.assertTrue(coverage["incomplete"])
                self.assertIsNone(coverage["next_cursor"])
                self.assertTrue(coverage["pagination_unavailable"])
                self.assertEqual(out["identity"]["head"], self.head)
                serialized = json.dumps(out)
                self.assertNotIn('bad-row-', serialized)
                cli = subprocess.run([sys.executable, str(identity.__file__), "--input-json", json.dumps({"path": str(self.wt)})], env={**os.environ, "HOME": str(self.home)}, check=True, capture_output=True, text=True, timeout=10)
                self.assertEqual(json.loads(cli.stdout)["identity"]["head"], self.head)

    def test_final_serialization_failure_is_structured(self) -> None:
        output = io.StringIO()
        with patch.object(identity, "inspect", return_value={"ok": True, "unexpected": b"not-json"}), patch.object(sys, "argv", ["reader", "--input-json", "{}"]), redirect_stdout(output):
            identity.main()
        result = json.loads(output.getvalue())
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"]["kind"], "serialization_error")

if __name__=="__main__":unittest.main()
