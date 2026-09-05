#!/usr/bin/env python3
"""Aggregate version-keyed reconciliation reports without forgetting failures."""
import argparse, json
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument("--reports", type=Path, required=True)
p.add_argument("--output", type=Path, required=True)
a = p.parse_args()
reports = []
for path in sorted(a.reports.glob("*/reconciliation.json")):
    try:
        report = json.loads(path.read_text())
    except Exception as error:
        report = {"complete": False, "error": str(error)}
    report["tag"] = path.parent.name
    reports.append(report)
complete = bool(reports) and all(row.get("complete") is True for row in reports)
payload = {"complete": complete, "versions": reports}
a.output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
print(json.dumps(payload, sort_keys=True))
raise SystemExit(0 if complete else 1)
