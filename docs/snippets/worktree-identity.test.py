#!/usr/bin/env python3
"""Disposable Git/SQLite fixtures: no network or existing task state changes."""
from typing import Any

import importlib.util
import json
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest

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

    def test_nullable_metadata_and_invalid_cursor_types(self) -> None:
        self.c.execute("UPDATE threads SET cwd=NULL,git_origin_url=NULL,git_branch=NULL")
        self.c.commit()
        self.assertEqual(self.run_identity()["sessions"]["verified"], [])
        for cursor in (None, 0, False, [], {}):
            with self.subTest(cursor=cursor):
                with self.assertRaises(self.reader.ContextError):
                    self.run_identity(session_cursor=cursor)

    def test_metadata_schema_failure_and_input_validation(self) -> None:
        self.c.execute("DROP TABLE threads")
        self.c.commit()
        self.assertFalse(self.run_identity()["sessions"]["coverage"]["available"])
        for values in ({"session_limit":0},{"session_cursor":"bad"},{"path":"relative"},{"extra":"command"}):
            with self.assertRaises(self.reader.ContextError):self.run_identity(**values)

if __name__=="__main__":unittest.main()
