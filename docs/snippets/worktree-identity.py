#!/usr/bin/env python3
"""Bounded read-only worktree/session metadata projection for a Labby snippet."""
from __future__ import annotations
import argparse
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import sys
from urllib.parse import urlsplit

VERSION = "1.0.0"
HELPER = Path.home() / ".local/share/labby/repo-onboarding/reader.py"
MAX_BYTES = 14000
KEY = re.compile(r"(?<![A-Za-z0-9])([A-Za-z][A-Za-z0-9]{1,11}-[1-9][0-9]{0,8})(?![A-Za-z0-9])")
PR = re.compile(r"https://github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/pull/([1-9][0-9]{0,8})(?![0-9])")

def refs(text, source):
    """Return handles only; arbitrary task text never becomes commands or output."""
    text = text[:16000] if isinstance(text, str) else ""
    return {"issue_keys": [{"key": x, "source": source} for x in sorted({x.upper() for x in KEY.findall(text)})[:8]],
            "pull_requests": [{"repository": f"{a}/{b}", "number": int(n), "url": f"https://github.com/{a}/{b}/pull/{n}", "source": source}
                              for a,b,n in sorted(set(PR.findall(text)))[:8]]}

def load_identity_reader(path=HELPER):
    spec = importlib.util.spec_from_file_location("labby_repository_onboarding", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("identity_reader_unavailable")
    reader = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(reader)
    original_remote_info=reader.remote_info
    def normalize_remote(raw):
        value=original_remote_info(raw)
        if value.get("github_repo") or not isinstance(raw,str):return value
        try:
            parsed=urlsplit(raw)
            host=(parsed.hostname or "").lower()
            repo=parsed.path.strip("/").removesuffix(".git")
            if parsed.scheme in ("ssh","https") and host in ("github.com","ssh.github.com") and re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+",repo):
                return {"url":"https://github.com/"+repo,"github_repo":repo}
        except ValueError:pass
        return value
    reader.remote_info=normalize_remote
    return reader

def canon(path):
    return str(Path(path).resolve(strict=False))

def same_or_child(path, root):
    return path == root or path.startswith(root.rstrip("/") + "/")

def registrations(reader, root):
    raw = reader.git(root, "worktree", "list", "--porcelain", "-z")
    if len(raw.encode()) > 262144:
        raise reader.ContextError("registration_budget", "Worktree registration exceeds 256 KiB")
    out, record = [], {}
    for field in raw.split("\0"):
        if not field:
            if record:
                out.append(record)
                record = {}
            continue
        k, _, v = field.partition(" ")
        if k == "worktree":
            record.update(path=canon(v))
        elif k == "HEAD": record["head"] = v
        elif k == "branch": record["branch"] = v.removeprefix("refs/heads/")
        elif k in ("detached", "bare", "locked", "prunable"):
            record[k] = True  # reasons can contain arbitrary private text
    if record: out.append(record)
    if len(out) > 256:
        raise reader.ContextError("registration_budget", "More than 256 registered worktrees")
    return out

def attachment_roots(kind, value):
    if not isinstance(value, dict): return []
    if kind in ("worktree", "pull_request"):
        return [value.get("root")]
    if kind == "archived_worktree" and isinstance(value.get("worktree"), dict):
        return [value["worktree"].get("root")]
    return []

def session_metadata(reader, home, identity, paths, limit, cursor):
    db = home / ".codex/state_5.sqlite"
    result = {"verified": [], "candidates": [], "issue_keys": [], "pull_requests": [],
              "coverage": {"source": str(db), "transcripts_read": False, "titles_returned": False}}
    if not db.is_file():
        result["coverage"].update(available=False, reason="metadata_database_missing")
        return result
    # mode=ro deliberately avoids immutable=1: include live WAL and do not modify it.
    c = None
    try:
        c = sqlite3.connect(db.as_uri() + "?mode=ro", uri=True, timeout=1)
        c.row_factory = sqlite3.Row
        c.set_progress_handler(lambda: 1, 2000000)
        cols = {r[1] for r in c.execute("PRAGMA table_info(threads)")}
        needed = {"id", "cwd", "git_sha", "git_branch", "git_origin_url", "archived", "title"}
        if not needed <= cols:
            raise sqlite3.DatabaseError("unsupported_thread_schema")
        # Focused SQL admits exact cwd/children, matching Git tuples, or explicit
        # worktree/PR attachments. Pagination is by stable ID, never timestamps.
        path_terms, args = [], [cursor]
        for p in paths:
            path_terms.extend(["cwd = ?", "substr(cwd,1,?) = ?"])
            args.extend([p, len(p.rstrip("/")+"/"), p.rstrip("/")+"/"])
        terms = list(path_terms)
        if identity.get("head"):
            terms.append("git_sha = ?")
            args.append(identity["head"])
        if identity.get("branch"):
            terms.append("git_branch = ?")
            args.append(identity["branch"])
        attachments = {r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if "thread_attachments" in attachments:
            root_terms, root_args = [], []
            for p in paths:
                for field in ("$.root", "$.worktree.root"):
                    root_terms.append("json_extract(payload,?) = ?")
                    root_args.extend([field,p])
            terms.append("id IN (SELECT thread_id FROM thread_attachments WHERE json_valid(payload) AND (" + " OR ".join(root_terms) + "))")
            args.extend(root_args)
        query = "SELECT id,cwd,git_sha,git_branch,git_origin_url,archived,substr(title,1,1000) AS title FROM threads WHERE id > ? AND (" + " OR ".join(terms) + ") ORDER BY id LIMIT ?"
        scan_limit = 200
        rows = c.execute(query, [*args, scan_limit + 1]).fetchall()
        scanned = 0
        for row in rows[:scan_limit]:
            sid = row["id"]
            scanned += 1
            if not re.fullmatch(r"[0-9a-fA-F-]{36}", sid): continue
            cwd = row["cwd"]
            evidence = []
            exact = isinstance(cwd,str) and cwd.startswith("/") and any(same_or_child(canon(cwd), p) for p in paths)
            if exact:
                evidence.append({"source":str(db),"record":sid,"field":"threads.cwd","match":"canonical_worktree_path"})
            attached = []
            if "thread_attachments" in attachments:
                attached = c.execute("SELECT attachment_type,payload FROM thread_attachments WHERE thread_id=? AND attachment_type IN ('worktree','archived_worktree','pull_request') ORDER BY id LIMIT 33", (sid,)).fetchall()
            hints = refs(row["title"], f"threads:{sid}:title")
            for a in attached[:32]:
                if len(a["payload"].encode()) > 16000: continue
                try: value = json.loads(a["payload"])
                except ValueError: continue
                roots = attachment_roots(a["attachment_type"],value)
                match = any(isinstance(p,str) and p.startswith("/") and canon(p) in paths for p in roots)
                if match:
                    exact = True
                    evidence.append({"source":str(db),"record":sid,"field":f"thread_attachments.{a['attachment_type']}.root","match":"canonical_worktree_path"})
                    if a["attachment_type"] == "pull_request":
                        hints["pull_requests"].extend(refs(value.get("url"),f"attachments:{sid}:pull_request.url")["pull_requests"])
                    if a["attachment_type"] == "archived_worktree":
                        for pr in value.get("pullRequests",[])[:8]:
                            if isinstance(pr,dict): hints["pull_requests"].extend(refs(pr.get("url"),f"attachments:{sid}:archived_worktree.pullRequests")["pull_requests"])
            repo = reader.remote_info(row["git_origin_url"])["github_repo"]
            tuple_match = repo is not None and repo in identity["repository_candidates"] and (
                row["git_sha"] == identity.get("head") or (identity.get("branch") and row["git_branch"] == identity["branch"]))
            if not exact and not tuple_match: continue
            item = {"id":sid,"archived":bool(row["archived"]),"confidence":"high" if exact else "candidate",
                    "association":"recorded_path" if exact else "git_metadata_only","evidence":evidence,
                    "git_branch":row["git_branch"],"git_head":row["git_sha"],
                    "historical_identity_differs":row["git_sha"] is not None and row["git_sha"] != identity.get("head")}
            if not exact:
                item["evidence"]=[{"source":str(db),"record":sid,"field":"threads.git_origin_url/git_sha/git_branch","match":"repository_and_head_or_branch"}]
            bucket = "verified" if exact else "candidates"
            if len(result["verified"]) + len(result["candidates"]) >= limit:
                scanned -= 1
                break
            result[bucket].append(item)
            if exact:
                result["issue_keys"].extend(hints["issue_keys"])
                result["pull_requests"].extend(hints["pull_requests"])
        next_cursor = rows[scanned-1]["id"] if scanned and (scanned < len(rows)) else None
        result["coverage"].update(available=True,scanned=scanned,scan_limit=scan_limit,next_cursor=next_cursor,
                                  incomplete=next_cursor is not None,attachments_per_session_limit=32,
                                  alias_limit="Only canonical/input spellings are selected by SQL; arbitrary historical symlink aliases may be absent",
                                  association_limit="Recorded path proves a historical association, not exclusive/current ownership; Git-only matches cannot prove relocation")
        result["issue_keys"] = result["issue_keys"][:16]
        result["pull_requests"] = result["pull_requests"][:16]
        return result
    except sqlite3.Error as e:
        result["coverage"].update(available=False,reason="metadata_read_failed",error_type=type(e).__name__)
        return result
    finally:
        if c is not None:c.close()

def inspect(inp, *, reader=None, home=None, hostname=None):
    home = (home or Path.home()).resolve()
    reader = reader or load_identity_reader()
    host = hostname or socket.gethostname()
    if host.lower().split(".")[0] != "macpoo":
        raise reader.ContextError("wrong_host", "Expected the Mac identity host; no repository or session reads were made")
    allowed = {"path","repo_hint","session_limit","session_cursor"}
    if not isinstance(inp,dict) or set(inp)-allowed:
        raise reader.ContextError("invalid_input", "Unknown input field")
    arg = reader.clean_string(inp.get("path"),"path")
    if not (arg.startswith("/") or arg.startswith("~/")):
        raise reader.ContextError("invalid_path","Use an absolute or ~/ path")
    lexical = os.path.abspath(str(home / arg[2:] if arg.startswith("~/") else Path(arg)))
    target = Path(lexical).resolve(strict=False)
    limit = inp.get("session_limit",10)
    cursor = inp.get("session_cursor","")
    if type(limit) is not int or not 1 <= limit <= 20:
        raise reader.ContextError("invalid_input","session_limit must be 1..20")
    if cursor and not re.fullmatch(r"[0-9a-fA-F-]{36}",cursor):
        raise reader.ContextError("invalid_input","session_cursor must be the returned session ID")
    if target.exists():
        if not target.is_dir(): raise reader.ContextError("invalid_path","path must be a directory")
        root,info = reader.repository_info(target,None)
        records = registrations(reader,root)
        found = next((r for r in records if r["path"] == str(root)),None)
        state = "live"
    else:
        hint = inp.get("repo_hint")
        if hint:
            reader.clean_string(hint,"repo_hint")
            if not (hint.startswith("/") or hint.startswith("~/")):
                raise reader.ContextError("invalid_path","repo_hint must be absolute or ~/path")
            anchor = (home / hint[2:] if hint.startswith("~/") else Path(hint)).resolve(strict=True)
        else:
            anchor=target.parent
            while not anchor.exists() and anchor != anchor.parent: anchor=anchor.parent
        root,info=reader.repository_info(anchor,None)
        records=registrations(reader,root)
        found=next((r for r in records if r["path"] == str(target)),None)
        if found is None:
            raise reader.ContextError("missing_unregistered_path","No exact worktree registration for this missing path; supply the owning repository as repo_hint. Renamed paths are not guessed")
        info.update(root=str(target),head=found.get("head"),branch=found.get("branch"),detached=bool(found.get("detached")),dirty=None,changes_count=None)
        info["git_dir"]=None
        state="missing_registered"
    repos = sorted({v["github_repo"] for r in info["remotes"] for v in r["fetch"] if v["github_repo"]})
    info["repository_candidates"]=repos
    identity={k:info.get(k) for k in ("root","common_dir","git_dir","head","branch","detached","dirty","changes_count","remotes","repository_candidates")}
    identity.update(path=str(target),input_path=lexical,path_state=state,git_identity_source="live_git" if state=="live" else "git_worktree_registration",
                    confidence="exact",registration=found,repository_selection=info.get("github_repo"),multiple_remotes=len(repos)>1)
    sessions=session_metadata(reader,home,identity,{str(Path(info["root"])),lexical} if state=="live" else {str(target),lexical},limit,cursor)
    if state == "live":
        current_head=reader.git(root,"rev-parse","--verify","HEAD",optional=True)
        current_branch=reader.git(root,"symbolic-ref","--quiet","--short","HEAD",optional=True)
        if (current_head,current_branch)!=(identity["head"],identity["branch"]):
            raise reader.ContextError("identity_changed","Git identity changed during inspection; run again")
    branch_refs=refs(identity.get("branch"),"git:HEAD:branch")
    return {"ok":True,"version":VERSION,"host":host,"observed_at":dt.datetime.now(dt.timezone.utc).isoformat(),"identity":identity,
            "sessions":{k:sessions[k] for k in ("verified","candidates","coverage")},
            "hints":{"issue_keys":branch_refs["issue_keys"]+sessions["issue_keys"],"pull_requests":sessions["pull_requests"]},
            "worktrees":{"total":len(records),"items":records[:24],"truncated":len(records)>24},
            "limits":{"network_contacted":False,"transcripts_read":False,"output_bytes":MAX_BYTES,"git_registration_limit":256}}

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--input-json",required=True)
    ns=p.parse_args()
    try:
        if len(ns.input_json.encode())>8000: raise ValueError("input_budget")
        result=inspect(json.loads(ns.input_json))
    except Exception as e:
        # Do not expose exception messages containing filesystem contents/URLs.
        result={"ok":False,"error":{"kind":getattr(e,"kind",type(e).__name__),"message":"Read-only identity/metadata inspection failed; verify path, host and existing helper permissions"}}
    encoded=json.dumps(result,ensure_ascii=True,separators=(",",":"))
    if len(encoded.encode())>MAX_BYTES:
        result["worktrees"]["items"]=[]
        result["worktrees"]["truncated"]=True
        encoded=json.dumps(result,ensure_ascii=True,separators=(",",":"))
    if len(encoded.encode())>MAX_BYTES:
        encoded=json.dumps({"ok":False,"error":{"kind":"output_budget","message":"Reduce session_limit and paginate"}})
    print(encoded)

if __name__ == "__main__": main()
