#!/usr/bin/env python3
"""Validate the dated MCP denominator and execute independently reviewed oracles.

Catalog validity is not compliance. `check` validates intent; `run` executes
exact test selectors and emits evidence; `report` rejects missing/stale evidence.
No outcome is stored in the committed requirement or oracle catalogs.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile

if __package__:
    from .extract_mcp_spec_requirements import extract
else:
    from extract_mcp_spec_requirements import extract

ROOT = Path(__file__).resolve().parents[2]
VERSION = "2026-07-28"
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
REVISION = re.compile(r"[0-9a-f]{40}\Z")
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.:-]*\Z")
STRENGTHS = {"must", "must_not", "should", "should_not", "may", "recommended", "not_recommended", "required", "shall", "shall_not", "optional"}
APPLICABILITY = {"applicable", "not_applicable", "conditional", "unreviewed"}
LEVELS = {"sdk_unit", "product_unit", "product_integration", "product_wire", "actual_host", "manual_review"}
ROLES = {"client", "server", "host", "authorization_server", "proxy", "unspecified"}
TRANSPORTS = {"http", "stdio", "all", "custom"}


class InvalidCatalog(ValueError):
    """Invalid or unverifiable compliance intent/evidence."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise InvalidCatalog(message)


def digest(value: object) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def index(items: list[dict], key: str, label: str) -> dict[str, dict]:
    require(isinstance(items, list), f"{label} must be a list")
    result = {}
    for item in items:
        require(isinstance(item, dict), f"invalid {label} entry")
        identity = item.get(key)
        require(isinstance(identity, str) and bool(identity), f"missing {label} {key}")
        require(identity not in result, f"duplicate {label}: {identity}")
        result[identity] = item
    return result


def safe_relative(path: str) -> bool:
    return isinstance(path, str) and bool(path) and not Path(path).is_absolute() and ".." not in Path(path).parts


def validate_catalog(sources: dict, catalog: dict) -> dict[str, dict]:
    for label, document in (("sources", sources), ("requirements", catalog)):
        require(document.get("schema_version") == 1, f"unsupported {label} schema")
        require(document.get("protocol_version") == VERSION, f"wrong {label} protocol version")
    revision = sources.get("revision", "")
    require(bool(REVISION.fullmatch(revision)), "source revision must be an immutable Git SHA")
    require(catalog.get("source_revision") == revision, "source revision mismatch")
    require(sources.get("repository") == "https://github.com/modelcontextprotocol/modelcontextprotocol", "unexpected specification repository")
    pages = index(sources.get("pages", []), "path", "source page")
    require(bool(pages), "empty source denominator")
    for path, page in pages.items():
        require(safe_relative(path), f"unsafe source path: {path}")
        require(bool(SHA256.fullmatch(page.get("sha256", ""))), f"invalid source hash: {path}")
        require(isinstance(page.get("normative_count"), int) and page["normative_count"] >= 0, f"invalid count: {path}")
        require(path.startswith(f"docs/specification/{VERSION}/") and path.endswith(".mdx"), f"wrong dated source path: {path}")
        require(page.get("url") == f"{sources['repository']}/blob/{revision}/{path}", f"wrong immutable URL: {path}")
        require(page.get("classification") == ("normative" if page["normative_count"] else "nonnormative"), f"wrong page classification: {path}")
    rows = index(catalog.get("requirements", []), "id", "requirement")
    require(bool(rows), "empty requirement denominator")
    counts: Counter[str] = Counter()
    for identity, row in rows.items():
        require(bool(IDENTIFIER.fullmatch(identity)), f"invalid requirement id: {identity}")
        path = row.get("source_path")
        require(path in pages, f"unknown source page for {identity}")
        require(row.get("source_url", "").split("#")[0] == pages[path]["url"].split("#")[0], f"source URL mismatch: {identity}")
        require(bool(SHA256.fullmatch(row.get("source_sha256", ""))), f"invalid clause hash: {identity}")
        lines = row.get("source_lines")
        require(isinstance(lines, list) and len(lines) == 2 and all(type(n) is int for n in lines) and 0 < lines[0] <= lines[1], f"invalid source lines: {identity}")
        require(row.get("strength") in STRENGTHS, f"invalid requirement strength: {identity}")
        require(isinstance(row.get("requirement"), str) and bool(row["requirement"].strip()), f"empty requirement: {identity}")
        require(row.get("applicability") in APPLICABILITY, f"invalid applicability: {identity}")
        require(isinstance(row.get("applicability_reason"), str) and bool(row["applicability_reason"].strip()), f"missing applicability rationale: {identity}")
        for field in ("roles", "transports"):
            require(isinstance(row.get(field), list) and bool(row[field]) and all(isinstance(v, str) and v for v in row[field]), f"missing {field}: {identity}")
            choices = ROLES if field == "roles" else TRANSPORTS
            require(set(row[field]) <= choices and len(row[field]) == len(set(row[field])), f"invalid {field}: {identity}")
        require(row.get("role_inference") in {"explicit_actor", "unspecified_actor"}, f"invalid role inference: {identity}")
        require((row["role_inference"] == "unspecified_actor") == (row["roles"] == ["unspecified"]), f"contradictory role inference: {identity}")
        require(isinstance(row.get("capability"), str) and bool(re.fullmatch(r"[a-z][a-z_]*", row["capability"])), f"invalid capability: {identity}")
        require(isinstance(row.get("source_anchor"), str) and row["source_anchor"].startswith("#") and len(row["source_anchor"]) > 1, f"missing source anchor: {identity}")
        require(row.get("requirement_kind") in {"direct", "external_reference", "unresolved_reference"}, f"invalid requirement kind: {identity}")
        refs = row.get("external_normative_references")
        require(isinstance(refs, list) and all(isinstance(ref, str) and ref.startswith("https://") for ref in refs), f"invalid external references: {identity}")
        require((row["requirement_kind"] == "direct") == (not refs), f"contradictory external references: {identity}")
        auth_ids = row.get("existing_auth_ids")
        require(isinstance(auth_ids, list) and len(set(auth_ids)) == len(auth_ids) and all(isinstance(ref, str) and ref.startswith("MCP-2026-AUTH-") for ref in auth_ids), f"invalid auth references: {identity}")
        require("status" not in row, f"static outcome prohibited: {identity}")
        counts[path] += 1
    require(all(counts[path] == page["normative_count"] for path, page in pages.items()), "source/requirement denominator mismatch")
    return rows


def oracle_command(oracle: dict) -> list[str]:
    """Construct commands, never execute catalog-supplied shell fragments."""
    kind = oracle.get("kind")
    test = oracle.get("test", "")
    require(isinstance(test, str) and bool(IDENTIFIER.fullmatch(test)), "oracle needs an exact test identifier")
    if kind == "cargo_nextest":
        package = oracle.get("package", "")
        require(bool(re.fullmatch(r"labby(?:-[a-z0-9-]+)?", package)), "oracle package must be a Labby crate")
        target = oracle.get("target", "")
        require(target == "lib" or bool(re.fullmatch(r"test:[a-z0-9_]+", target)), "oracle needs explicit lib/test target")
        target_args = ["--lib"] if target == "lib" else ["--test", target[5:]]
        return ["cargo", "nextest", "run", "--locked", "--all-features", "-p", package, *target_args, "--no-tests=fail", "-E", f"test(={test})"]
    if kind == "python_unittest":
        require(bool(re.fullmatch(r"scripts\.ci\.test_[A-Za-z0-9_]+\.[A-Za-z0-9_]+\.test_[A-Za-z0-9_]+", test)), "Python oracle must name one repository unittest method")
        return ["python3", "scripts/ci/mcp_oracle_runner.py", test]
    raise InvalidCatalog(f"unsupported executable oracle kind: {kind}")


def validate_oracles(document: dict, rows: dict[str, dict]) -> dict[str, dict]:
    require(document.get("schema_version") == 1 and document.get("protocol_version") == VERSION, "wrong oracle schema/version")
    oracles = index(document.get("oracles", []), "id", "oracle")
    for identity, oracle in oracles.items():
        require(bool(IDENTIFIER.fullmatch(identity)), f"invalid oracle id: {identity}")
        refs = oracle.get("requirement_ids")
        require(isinstance(refs, list) and bool(refs) and len(set(refs)) == len(refs), f"invalid oracle requirement references: {identity}")
        require(all(ref in rows for ref in refs), f"unknown oracle requirement: {identity}")
        require(isinstance(oracle.get("expected"), str) and bool(oracle["expected"].strip()), f"missing independent expectation: {identity}")
        require(oracle.get("evidence_level") in LEVELS, f"missing evidence level: {identity}")
        scopes = oracle.get("scopes")
        require(isinstance(scopes, list) and bool(scopes), f"missing oracle scopes: {identity}")
        for scope in scopes:
            require(isinstance(scope, dict) and set(scope) == {"role", "transport"}, f"invalid oracle scope: {identity}")
            require(scope["role"] in ROLES - {"unspecified"} and scope["transport"] in TRANSPORTS - {"all"}, f"invalid oracle scope: {identity}")
        require(type(oracle.get("timeout_seconds")) is int and 0 < oracle["timeout_seconds"] <= 1800, f"invalid oracle deadline: {identity}")
        require("status" not in oracle, f"static outcome prohibited: {identity}")
        oracle_command(oracle)
    return oracles


def schema_requirements(document: dict, checkout: Path | None) -> dict[str, dict]:
    """Project separately pinned machine constraints into the same coverage gate."""
    require(document.get("schema_version") == 1 and document.get("protocol_version") == VERSION, "wrong machine-schema inventory version")
    require(document.get("source_revision") == "5f5440bb26a62e2cf3440b92da5a667efa03b267", "wrong machine-schema revision")
    require(document.get("source_path") == f"schema/{VERSION}/schema.json", "wrong machine-schema path")
    require(bool(SHA256.fullmatch(document.get("source_sha256", ""))), "invalid machine-schema source hash")
    require(bool(index(document.get("definitions", []), "id", "schema definition")), "empty schema definitions")
    constraints = index(document.get("constraints", []), "id", "schema constraint")
    require(bool(constraints), "empty schema constraints")
    if __package__:
        from .extract_mcp_schema_requirements import canonical, extract as extract_schema, row_id
    else:
        from extract_mcp_schema_requirements import canonical, extract as extract_schema, row_id
    if checkout is not None:
        require(document == extract_schema(checkout), "machine-schema inventory differs from deterministic extraction")
    rows = {}
    for identity, constraint in constraints.items():
        require(bool(IDENTIFIER.fullmatch(identity)), "invalid schema constraint ID")
        pointer = constraint.get("pointer")
        require(isinstance(pointer, str) and pointer.startswith("#/"), f"invalid schema pointer: {identity}")
        require(identity == row_id("MCP-SCHEMA-CONSTRAINT", pointer), f"schema constraint ID mismatch: {identity}")
        require(isinstance(constraint.get("keyword"), str) and bool(constraint["keyword"]), f"invalid schema keyword: {identity}")
        value_hash = hashlib.sha256(canonical(constraint.get("value"))).hexdigest()
        require(constraint.get("value_sha256") == value_hash, f"schema value hash mismatch: {identity}")
        constraint_hash = hashlib.sha256(canonical([pointer, constraint["keyword"], value_hash])).hexdigest()
        require(constraint.get("constraint_sha256") == constraint_hash, f"schema constraint hash mismatch: {identity}")
        require("status" not in constraint, f"static schema outcome prohibited: {identity}")
        rows[identity] = {
            "id": identity, "source_path": document["source_path"],
            "source_url": f"https://github.com/modelcontextprotocol/modelcontextprotocol/blob/{document['source_revision']}/{document['source_path']}",
            "source_sha256": document["source_sha256"], "source_pointer": pointer,
            "requirement": f"Structural schema constraint at {pointer}: {json.dumps(constraint['value'], sort_keys=True)}",
            "strength": "must", "requirement_kind": "structural_schema",
            "roles": ["unspecified"], "transports": ["all"],
            "applicability": "unreviewed",
            "applicability_reason": "Structural constraint requires reviewed product role and transport applicability; inventory is not execution evidence.",
        }
    return rows


def apply_dispositions(document: dict, rows: dict[str, dict]) -> dict[str, dict]:
    """Keep reviewed product applicability separate from generated source facts.

    Each required evidence cell is explicit: a server HTTP test does not also
    qualify the client, proxy, or stdio surface. Missing reviews remain gaps.
    """
    require(document.get("schema_version") == 1 and document.get("protocol_version") == VERSION, "wrong disposition schema/version")
    decisions = index(document.get("dispositions", []), "requirement_id", "disposition")
    require(set(decisions) <= set(rows), "unknown disposition requirement")
    result = {key: dict(row) for key, row in rows.items()}
    for identity, decision in decisions.items():
        row = result[identity]
        require(decision.get("requirement_sha256") == digest(rows[identity]), f"stale disposition: {identity}")
        require(decision.get("applicability") in {"applicable", "not_applicable", "conditional"}, f"invalid disposition: {identity}")
        for field in ("rationale", "review_reference"):
            require(isinstance(decision.get(field), str) and bool(decision[field].strip()), f"missing disposition {field}: {identity}")
        cells = decision.get("required_evidence")
        require(isinstance(cells, list), f"missing evidence cells: {identity}")
        require(decision["applicability"] != "applicable" or bool(cells), f"applicable requirement without evidence cells: {identity}")
        require(decision["applicability"] != "not_applicable" or not cells, f"nonapplicable requirement has evidence cells: {identity}")
        unique = set()
        for cell in cells:
            require(isinstance(cell, dict), f"invalid evidence cell: {identity}")
            role, transport, level = cell.get("role"), cell.get("transport"), cell.get("level")
            require(role in ROLES - {"unspecified"} and transport in TRANSPORTS - {"all"} and level in LEVELS, f"invalid evidence cell: {identity}")
            key = (role, transport, level)
            require(key not in unique, f"duplicate evidence cell: {identity}")
            unique.add(key)
        row.update(applicability=decision["applicability"], applicability_reason=decision["rationale"], required_evidence=cells)
    return result


def evidence_cells_satisfied(row: dict, linked: list[str], oracles: dict) -> bool:
    """Exact levels, not an invented ranking of unit vs host evidence."""
    return all(any(
        oracle.get("evidence_level") == cell["level"]
        and {"role": cell["role"], "transport": cell["transport"]} in oracle.get("scopes", [])
        for oracle in (oracles[key] for key in linked)
    ) for cell in row.get("required_evidence", []))


def source_check(sources: dict, catalog: dict, checkout: Path) -> None:
    """Verify the immutable checkout and every page/clause byte hash offline."""
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout, text=True).strip()
    require(revision == sources["revision"], "spec checkout revision mismatch")
    actual_paths = {str(path.relative_to(checkout)) for path in (checkout / f"docs/specification/{VERSION}").rglob("*.mdx")}
    require(actual_paths == {page["path"] for page in sources["pages"]}, "spec page denominator differs from checkout")
    for page in sources["pages"]:
        data = (checkout / page["path"]).read_bytes()
        committed = subprocess.check_output(["git", "show", f"{revision}:{page['path']}"], cwd=checkout)
        require(data == committed, f"source differs from pinned Git blob: {page['path']}")
        require(hashlib.sha256(data).hexdigest() == page["sha256"], f"source content drift: {page['path']}")
    for row in catalog["requirements"]:
        lines = (checkout / row["source_path"]).read_bytes().splitlines(keepends=True)
        first, last = row["source_lines"]
        require(last <= len(lines), f"clause beyond source: {row['id']}")
        snippet = b"".join(lines[first - 1:last])
        require(hashlib.sha256(snippet).hexdigest() == row["source_sha256"], f"clause drift: {row['id']}")
    # Counts/hashes alone do not prove completeness: a row could be deleted
    # together with its page count. Re-extract the immutable source and compare
    # the entire derived denominator, including wording and inferred metadata.
    regenerated_sources, regenerated_catalog = extract(checkout, ROOT / "conformance/mcp-auth-normative.json")
    require(sources == regenerated_sources, "source inventory differs from deterministic extraction")
    require(catalog == regenerated_catalog, "requirement inventory differs from deterministic extraction")


def worktree_fingerprint(root: Path) -> str:
    """Bind results to actual source bytes, including uncommitted test additions."""
    names = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root).split(b"\0")
    result = hashlib.sha256()
    for name in sorted(set(names) - {b""}):
        path = root / name.decode()
        result.update(name + b"\0")
        if path.is_symlink():
            result.update(b"symlink:" + str(path.readlink()).encode())
        elif path.is_file():
            result.update(path.read_bytes())
        else:
            result.update(b"deleted")
        result.update(b"\0")
    return result.hexdigest()


def dependency_fingerprint(root: Path) -> str:
    """Bind Cargo resolution and local dependency bytes, including SDK patches.

    Published dependencies are identified by locked source IDs/checksums. Local
    path dependencies must belong to a Git worktree so ignored build outputs
    cannot make every receipt immediately stale. Hash the complete dependency
    checkout, including dirty and untracked files, not only Cargo.toml.
    """
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--offline", "--all-features", "--format-version", "1"], cwd=root, text=True,
    ))
    paths = {}
    packages = []
    for package in metadata["packages"]:
        packages.append({key: package[key] for key in ("id", "source", "version")})
        if package["source"] is not None:
            continue
        parent = Path(package["manifest_path"]).resolve().parent
        if parent.is_relative_to(root.resolve()):
            continue
        checkout = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], cwd=parent, text=True).strip()).resolve()
        require(checkout != root.resolve(), "external dependency symlink escapes source fingerprint")
        if str(checkout) not in paths:
            paths[str(checkout)] = worktree_fingerprint(checkout)
    toolchain = subprocess.check_output(["rustc", "-vV"], cwd=root, text=True)
    return digest({"packages": sorted(packages, key=lambda item: item["id"]), "resolve": metadata["resolve"], "external_paths": paths, "rustc": toolchain})


def execute(command: list[str], timeout: int) -> tuple[int | None, str]:
    """Bound the whole test process group, not just the Cargo parent process."""
    require(os.name == "posix", "oracle execution currently requires POSIX process-group containment")
    process = subprocess.Popen(command, cwd=ROOT, start_new_session=True)
    try:
        code = process.wait(timeout=timeout)
        return code, "passed" if code == 0 else "failed"
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        finally:
            # Descendants can outlive a terminated Cargo parent.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
        return None, "timeout"


def invalidate_evidence(receipt: Path, report: Path) -> None:
    require(receipt.resolve() != report.resolve(), "receipt and report paths must differ")
    # Only the two declared outputs, never the directory or unrelated artifacts.
    receipt.unlink(missing_ok=True)
    report.unlink(missing_ok=True)


def write_json(path: Path, value: dict) -> None:
    """An interrupted write must not leave a partial JSON evidence document."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, prefix=".mcp-evidence-", delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(value, stream, indent=2)
            stream.write("\n")
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def coverage(rows: dict, oracles: dict, receipt: dict | None, binding: dict) -> dict:
    results = {}
    if receipt is not None:
        require(receipt.get("schema_version") == 1 and receipt.get("binding") == binding, "stale or mismatched execution receipt")
        results = index(receipt.get("results", []), "oracle_id", "result")
        require(set(results) <= set(oracles), "receipt references unknown oracle")
        for identity, result in results.items():
            require(result.get("command") == oracle_command(oracles[identity]), f"receipt command mismatch: {identity}")
            require(result.get("outcome") in {"passed", "failed", "timeout"}, f"invalid receipt outcome: {identity}")
            code = result.get("returncode")
            valid_code = {"passed": type(code) is int and code == 0, "failed": type(code) is int and code != 0, "timeout": code is None}[result["outcome"]]
            require(valid_code, f"inconsistent receipt outcome: {identity}")
    requirement_results = []
    for identity, row in rows.items():
        linked = [key for key, oracle in oracles.items() if identity in oracle["requirement_ids"]]
        if row["applicability"] == "not_applicable":
            outcome = "not_applicable"
        elif row["applicability"] in {"unreviewed", "conditional"}:
            outcome = "applicability_unresolved"
        elif not linked:
            outcome = "missing_oracle"
        elif not evidence_cells_satisfied(row, linked, oracles):
            outcome = "insufficient_evidence_scope"
        elif any(results.get(key, {}).get("outcome") in {"failed", "timeout"} for key in linked):
            outcome = "failed"
        elif all(results.get(key, {}).get("outcome") == "passed" for key in linked):
            outcome = "passed"
        else:
            outcome = "not_run"
        requirement_results.append({"id": identity, "outcome": outcome, "oracle_ids": linked, "evidence_levels": sorted({oracles[key]["evidence_level"] for key in linked})})
    counts = dict(Counter(row["outcome"] for row in requirement_results))
    return {"schema_version": 1, "protocol_version": VERSION, "binding": binding, "counts": counts, "compliant": all(row["outcome"] in {"passed", "not_applicable"} for row in requirement_results), "requirements": requirement_results}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["check", "run", "report"])
    parser.add_argument("--spec-checkout", type=Path)
    parser.add_argument("--receipt", type=Path, default=ROOT / "target/mcp-spec-compliance/receipt.json")
    parser.add_argument("--output", type=Path, default=ROOT / "target/mcp-spec-compliance/report.json")
    parser.add_argument("--oracle", action="append", default=[])
    parser.add_argument("--gate", choices=["full", "oracles"], default="full", help="full requires complete compliance; oracles gates only every registered executable oracle and still reports coverage gaps")
    args = parser.parse_args()
    if args.mode == "run":
        # Do this even if subsequent source validation/fetch fails: a failed
        # attempt must not upload an earlier run's successful documents.
        invalidate_evidence(args.receipt, args.output)
    sources = json.loads((ROOT / "conformance/mcp-spec-sources.json").read_text())
    catalog = json.loads((ROOT / "conformance/mcp-spec-requirements.json").read_text())
    mapping = json.loads((ROOT / "conformance/mcp-spec-oracles.json").read_text())
    dispositions = json.loads((ROOT / "conformance/mcp-spec-dispositions.json").read_text())
    schema_catalog = json.loads((ROOT / "conformance/mcp-spec-schema.json").read_text())
    rows = validate_catalog(sources, catalog)
    if args.spec_checkout:
        source_check(sources, catalog, args.spec_checkout)
    structural_rows = schema_requirements(schema_catalog, args.spec_checkout)
    require(not (set(rows) & set(structural_rows)), "prose and structural requirement IDs collide")
    rows.update(structural_rows)
    oracles = validate_oracles(mapping, rows)
    rows = apply_dispositions(dispositions, rows)
    if args.mode == "check":
        source_state = "source regenerated" if args.spec_checkout else "schema only; source completeness NOT checked"
        print(f"Valid intent ({source_state}): {len(rows)} requirements, {len(oracles)} oracles. Not a compliance result.")
        return 0
    require(args.spec_checkout is not None, "run/report require --spec-checkout to verify the complete source denominator")
    require(args.gate == "full" or (args.mode == "run" and not args.oracle), "oracle-only gate requires run of the complete registered oracle set")
    binding = {"catalog_sha256": digest([sources, catalog, schema_catalog, mapping, dispositions]), "worktree_sha256": worktree_fingerprint(ROOT), "dependencies_sha256": dependency_fingerprint(ROOT)}
    receipt = None
    if args.mode == "run":
        selected = args.oracle or list(oracles)
        require(bool(selected) and set(selected) <= set(oracles), "empty or unknown oracle selection")
        results = []
        receipt = {"schema_version": 1, "binding": binding, "results": results}
        write_json(args.receipt, receipt)
        write_json(args.output, coverage(rows, oracles, receipt, binding))
        for identity in dict.fromkeys(selected):
            oracle = oracles[identity]
            command = oracle_command(oracle)
            print(f"Running {identity}", flush=True)
            # Output stays in the terminal rather than being copied into retained
            # evidence, which could otherwise persist credentials from a failure.
            code, outcome = execute(command, oracle["timeout_seconds"])
            result = {"oracle_id": identity, "command": command, "returncode": code, "outcome": outcome}
            results.append(result)
        require(worktree_fingerprint(ROOT) == binding["worktree_sha256"], "source changed during oracle execution; evidence rejected")
        require(dependency_fingerprint(ROOT) == binding["dependencies_sha256"], "dependencies changed during oracle execution; evidence rejected")
        receipt = {"schema_version": 1, "binding": binding, "results": results}
        write_json(args.receipt, receipt)
    elif args.receipt.exists():
        receipt = json.loads(args.receipt.read_text())
    report = coverage(rows, oracles, receipt, binding)
    write_json(args.output, report)
    mapped_passed = bool(oracles) and all(row["outcome"] == "passed" for row in report["requirements"] if row["oracle_ids"])
    print(json.dumps({"gate": args.gate, "compliant": report["compliant"], "registered_oracles_passed": mapped_passed, "counts": report["counts"]}, sort_keys=True))
    return 0 if (report["compliant"] if args.gate == "full" else mapped_passed) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (InvalidCatalog, OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        print(f"MCP compliance: {error}", file=sys.stderr)
        raise SystemExit(2) from error
