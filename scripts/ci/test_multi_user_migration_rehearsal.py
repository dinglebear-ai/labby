import copy
import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("validate-multi-user-migration-rehearsal.py")
SPEC = importlib.util.spec_from_file_location("migration_rehearsal", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

ZERO = "0" * 64
ONE = "1" * 64


def row(name, *, count=2, quarantine=0):
    return {
        "class": name,
        "pre": {"count": count, "stableIdsSha256": ZERO, "contentSha256": ONE},
        "post": {"count": count, "stableIdsSha256": ZERO, "contentSha256": ONE},
        "expected": {
            "countDelta": 0,
            "preserveStableIds": True,
            "preserveContent": True,
            "quarantineCount": quarantine,
        },
    }


def valid_manifest():
    return {
        "schemaVersion": "labby.multi-user-migration-rehearsal/v1",
        "checkpointSha256": ZERO,
        "rollbackCheckpointSha256": ZERO,
        "systems": {
            system: {"inventory": [row(name) for name in sorted(classes)]}
            for system, classes in MODULE.REQUIRED_INVENTORIES.items()
        },
    }


class MigrationRehearsalTest(unittest.TestCase):
    def test_complete_preserving_manifest_passes(self):
        manifest = valid_manifest()
        jobs = next(
            item for item in manifest["systems"]["depot"]["inventory"] if item["class"] == "jobs"
        )
        jobs["expected"]["quarantineCount"] = 1
        MODULE.validate(manifest)

    def test_missing_inventory_class_fails_closed(self):
        manifest = valid_manifest()
        manifest["systems"]["depot"]["inventory"].pop()
        with self.assertRaisesRegex(ValueError, "class mismatch; missing="):
            MODULE.validate(manifest)

    def test_disappearing_records_are_rejected(self):
        manifest = valid_manifest()
        manifest["systems"]["labby"]["inventory"][0]["post"]["count"] -= 1
        with self.assertRaisesRegex(ValueError, "count delta"):
            MODULE.validate(manifest)

    def test_stable_id_and_content_drift_are_rejected(self):
        for field, message in (("stableIdsSha256", "stable IDs"), ("contentSha256", "content")):
            manifest = valid_manifest()
            manifest["systems"]["depot"]["inventory"][0]["post"][field] = "2" * 64
            with self.assertRaisesRegex(ValueError, message):
                MODULE.validate(manifest)

    def test_quarantine_is_bounded_and_limited_to_reviewed_classes(self):
        manifest = valid_manifest()
        manifest["systems"]["labby"]["inventory"][0]["expected"]["quarantineCount"] = 1
        with self.assertRaisesRegex(ValueError, "not an approved quarantine-bearing inventory"):
            MODULE.validate(manifest)

        manifest = valid_manifest()
        jobs = next(
            item for item in manifest["systems"]["depot"]["inventory"] if item["class"] == "jobs"
        )
        jobs["expected"]["quarantineCount"] = 3
        with self.assertRaisesRegex(ValueError, "exceeds the post-migration count"):
            MODULE.validate(manifest)

    def test_rollback_must_bind_the_exact_checkpoint(self):
        manifest = copy.deepcopy(valid_manifest())
        manifest["rollbackCheckpointSha256"] = "f" * 64
        with self.assertRaisesRegex(ValueError, "exactly match"):
            MODULE.validate(manifest)


if __name__ == "__main__":
    unittest.main()
