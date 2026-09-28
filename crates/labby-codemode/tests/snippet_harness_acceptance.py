"""Run offline command-contract tests against the compiled Rust harness example."""
from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: snippet_harness_acceptance.py /absolute/path/to/snippet_harness", file=sys.stderr)
        return 2
    executable = Path(sys.argv[1]).resolve(strict=True)
    fixtures = Path(__file__).parent / "fixtures" / "snippet-harness"
    source = Path(__file__).resolve().parents[3] / "docs/snippets/unraid-linear-pr-triage-v2.md"
    cases = [
        ("fast", source, "fast", {}, 0),
        ("diagnostics", source, "fast", {"diagnostics": True}, 0),
        ("deep", source, "deep", {"deep": True}, 0),
        ("pagination", source, "paginated", {}, 0),
        ("page budget", source, "page-budget", {"maxPages": 1}, 0),
        ("rate limit", source, "rate-limit", {}, 0),
        ("worker failure", source, "worker-failure", {}, 0),
        ("wrong assertion", source, "wrong-assertion", {}, 1),
        ("absent runtime APIs", fixtures / "no-runtime-apis.md", "no-runtime-apis", {}, 0),
        ("swallowed unexpected call", fixtures / "swallowed-unexpected.md", "empty", {}, 1),
        ("real host bridge denied", fixtures / "host-escape.md", "empty", {}, 1),
        ("malformed JavaScript", fixtures / "invalid-js.md", "empty", {}, 2),
        ("runaway deadline", fixtures / "timeout.md", "timeout", {}, 2),
        ("normalized snapshot", fixtures / "snapshot.md", "snapshot", {}, 0),
        ("invalid input before calls", source, "empty", {"chunkSize": 1.5}, 1),
    ]
    results = []
    for name, snippet, fixture, inputs, expected_exit in cases:
        try:
            proc = subprocess.run([str(executable), str(snippet), str(fixtures / (fixture + ".json")),
                                   json.dumps(inputs)], capture_output=True, text=True, timeout=8, check=False)
            report = json.loads(proc.stdout) if proc.stdout.strip() else None
            passed = proc.returncode == expected_exit
            if expected_exit < 2:
                passed = passed and isinstance(report, dict) and report.get("passed") is (expected_exit == 0)
                result = report.get("result") if isinstance(report, dict) else None
                if name == "fast":
                    passed = passed and isinstance(result, dict) and isinstance(result.get("timing", {}).get("totalMs"), int)
                if name == "diagnostics":
                    diagnostics = result.get("diagnostics") if isinstance(result, dict) else None
                    passed = passed and isinstance(diagnostics, dict) and isinstance(diagnostics.get("calls"), list)
                    passed = passed and result.get("mode", {}).get("diagnostics") is True
            else:
                passed = passed and bool(proc.stderr.strip())
            results.append({"case": name, "passed": passed, "exit": proc.returncode,
                            "metrics": report.get("metrics") if isinstance(report, dict) else None,
                            "failures": report.get("failures") if isinstance(report, dict) else proc.stderr.strip()[:500]})
        except (subprocess.TimeoutExpired, json.JSONDecodeError, OSError) as error:
            results.append({"case": name, "passed": False, "error": str(error)[:500]})
    print(json.dumps({"passed": all(row["passed"] for row in results),
                      "cases": len(results), "results": results}, indent=2))
    return 0 if all(row["passed"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
