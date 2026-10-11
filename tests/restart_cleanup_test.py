#!/usr/bin/env python3
"""Isolated ownership checks for restart smoke profile cleanup; no CEF needed."""

import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

from restart_smoke import cleanup_owned_profile
from smoke import CheckFailed


class RestartCleanupTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="restart-cleanup-test-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.data = self.base / "owned-run"
        self.data.mkdir()
        created = self.data.stat()
        self.identity = (created.st_dev, created.st_ino)
        self.token = "synthetic-test-run"
        self.marker = self.data / ".restart-smoke-owner.json"
        self.marker.write_text(json.dumps({"test": "restart_smoke", "run": self.token}))
        self.profile = self.data / "profile-v1"
        self.profile.mkdir()
        (self.profile / "synthetic-cache").write_text("disposable fixture")
        self.processes = [SimpleNamespace(poll=lambda: 0), SimpleNamespace(poll=lambda: 0)]

    def cleanup_profile(self):
        return cleanup_owned_profile(self.data, self.identity, self.token, self.processes)

    def test_success_removes_only_owned_profile_and_preserves_evidence(self):
        evidence = {
            "restart-report.json": "{}", "launch-1.log": "first launch",
            "launch-2.log": "second launch", "cycle-1/screenshot.png": "PNG fixture",
            "cycle-2/screenshot.png": "PNG fixture", "profile/user-data": "preserve",
        }
        for name, content in evidence.items():
            path = self.data / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        outside = self.base / "unrelated-data"
        outside.write_text("do not follow profile symlinks")
        (self.profile / "external-link").symlink_to(outside)
        result = self.cleanup_profile()
        self.assertEqual(result, {"path": str(self.profile), "removed": True})
        self.assertFalse(self.profile.exists())
        self.assertTrue(self.marker.is_file())
        for name, content in evidence.items():
            self.assertEqual((self.data / name).read_text(), content)
        self.assertEqual(outside.read_text(), "do not follow profile symlinks")

    def test_changed_owner_marker_preserves_profile(self):
        self.marker.write_text(json.dumps({"test": "restart_smoke", "run": "another-run"}))
        with self.assertRaisesRegex(CheckFailed, "owner marker"):
            self.cleanup_profile()
        self.assertTrue((self.profile / "synthetic-cache").is_file())

    def test_replaced_data_directory_preserves_both_profiles(self):
        original = self.base / "original-run"
        self.data.rename(original)
        self.data.mkdir()
        self.marker.write_text(json.dumps({"test": "restart_smoke", "run": self.token}))
        self.profile.mkdir()
        (self.profile / "replacement-cache").write_text("preserve replacement")
        with self.assertRaisesRegex(CheckFailed, "identity changed"):
            self.cleanup_profile()
        self.assertTrue((original / "profile-v1/synthetic-cache").is_file())
        self.assertTrue((self.profile / "replacement-cache").is_file())

    def test_symlinked_root_is_refused(self):
        original = self.base / "original-run"
        self.data.rename(original)
        self.data.symlink_to(original, target_is_directory=True)
        with self.assertRaisesRegex(CheckFailed, "symlink"):
            self.cleanup_profile()
        self.assertTrue((original / "profile-v1/synthetic-cache").is_file())

    def test_symlinked_profile_is_refused(self):
        outside = self.base / "separate-profile"
        self.profile.rename(outside)
        self.profile.symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(CheckFailed, "symlink"):
            self.cleanup_profile()
        self.assertTrue((outside / "synthetic-cache").is_file())

    def test_running_or_failed_process_preserves_profile(self):
        for status in (None, 1):
            with self.subTest(status=status):
                self.processes[1] = SimpleNamespace(poll=lambda: status)
                with self.assertRaisesRegex(CheckFailed, "confirmed successful exits"):
                    self.cleanup_profile()
                self.assertTrue((self.profile / "synthetic-cache").is_file())


if __name__ == "__main__":
    unittest.main()
