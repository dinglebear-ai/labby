#!/usr/bin/env python3
"""Independent source-contract oracles for the host update lifecycle."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def require(text: str, needle: str, message: str) -> None:
    if needle not in text:
        raise SystemExit(f"update safety oracle failed: {message}")


installer = (ROOT / "scripts/install.sh").read_text()
manual = (ROOT / "crates/labby/src/cli/update.rs").read_text()
shared = (ROOT / "crates/labby/src/self_update.rs").read_text()
service = (ROOT / "scripts/install-macos-service.sh").read_text()
workflow = (ROOT / ".github/workflows/ci.yml").read_text()
macos_job = workflow.split("  macos-installer:", 1)[1].split("\n  frontend-assets:", 1)[0]

require(manual, "self_update::install_requested_release", "manual updates must use the shared sanitized installer path")
for variable in ("RECOVER_ONLY", "ROLLBACK", "LOCAL_BINARY", "LOCAL_SHA256"):
    require(shared, f'"LABBY_INSTALL_{variable}"', f"{variable} must be explicitly removed from child control")
require(installer, "acquire_transaction_lock", "the installer must own a cross-entry-point lock")
require(installer, "durability_barrier", "activation must have an explicit persistence barrier")
require(installer, "prune_unreferenced_artifacts", "successful activation must bound artifact retention")
require(service, "restore_install_state", "failed scheduler transfer must restore the service transaction")
require(workflow, "cargo test -p labby --all-features --locked self_update::tests", "macOS CI must execute Rust updater tests")
require(workflow, "cargo test -p labby --all-features --locked --test gateway_auto_reconnect", "macOS CI must execute gateway recovery E2E")
require(macos_job, "needs.changes.outputs.rust_test == 'true'", "Rust changes must route through the macOS updater job")

copies = [ROOT / "install.sh", ROOT / "apps/gateway-admin/public/install.sh"]
if any(path.read_bytes() != (ROOT / "scripts/install.sh").read_bytes() for path in copies):
    raise SystemExit("update safety oracle failed: published installer copies diverged")

print("update safety source-contract oracles passed")
