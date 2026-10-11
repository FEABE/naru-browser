"""Exercise cleanup boundaries using temporary files and a mocked cargo command."""

import importlib.util
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "cleanup-build.py"
SPEC = importlib.util.spec_from_file_location("naru_cleanup_build", SCRIPT)
cleanup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cleanup)


class BuildCleanupTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="naru-cleanup-test-")
        self.addCleanup(temporary.cleanup)
        self.sandbox = Path(temporary.name).resolve()
        self.root = self.sandbox / "checkout"
        self.root.mkdir()
        self.target = self.root / "target"
        root_patch = mock.patch.object(cleanup, "ROOT", self.root)
        root_patch.start()
        self.addCleanup(root_patch.stop)
        # No test may execute cargo, even if a rejection guard regresses.
        cargo_patch = mock.patch.object(cleanup.subprocess, "run")
        self.cargo = cargo_patch.start()
        self.addCleanup(cargo_patch.stop)
        self.unrelated = {}
        for name in (
            "Cargo.toml", "src/main.rs", ".local-data/profile-v1/session.fixture",
            ".cache/cef/runtime.fixture", "dist/Light Browser.app/app.fixture",
        ):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            content = ("preserve " + name).encode()
            path.write_bytes(content)
            self.unrelated[path] = content

    def binaries(self, directory, label="current"):
        directory.mkdir(parents=True, exist_ok=True)
        contents = {}
        for name in cleanup.BINARIES:
            path = directory / name
            content = (label + " " + name).encode()
            path.write_bytes(content)
            path.chmod(0o751)
            contents[name] = content
        return contents

    def assert_unrelated_preserved(self):
        for path, content in self.unrelated.items():
            self.assertEqual(path.read_bytes(), content, str(path))

    def assert_binaries_preserved(self, profile, contents):
        for name, content in contents.items():
            path = self.target / profile / name
            self.assertEqual(path.read_bytes(), content)
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o751)

    def remove_fake_target(self, command, **kwargs):
        self.assertEqual(command, ["cargo", "clean", "--manifest-path", str(self.root / "Cargo.toml"),
                                   "--target-dir", str(self.target)])
        self.assertEqual(kwargs, {"check": True, "cwd": self.root})
        # The patched ROOT guarantees this is exclusively the test's temp tree.
        self.assertEqual(self.target.parent, self.root)
        shutil.rmtree(self.target)

    def test_preserves_three_current_binaries_and_removes_old_builds(self):
        contents = self.binaries(self.target / "release")
        self.binaries(self.target / "debug", "old")
        for name in ("release/deps/stale.rlib", "release/incremental/stale.o", "debug/deps/stale.rlib"):
            path = self.target / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"old intermediate")
        self.cargo.side_effect = self.remove_fake_target

        cleanup.clean_build(self.target, "release")

        self.cargo.assert_called_once()
        self.assert_binaries_preserved("release", contents)
        self.assertEqual({str(path.relative_to(self.target)) for path in self.target.rglob("*") if path.is_file()},
                         {"release/" + name for name in cleanup.BINARIES})
        self.assertFalse((self.target / "debug").exists())
        self.assert_unrelated_preserved()
        self.assertFalse(list(self.root.glob(".naru-build-keep-*")))

    def test_failed_clean_restores_current_binaries_even_after_target_removal(self):
        contents = self.binaries(self.target / "release")

        def failing_clean(command, **kwargs):
            self.remove_fake_target(command, **kwargs)
            raise subprocess.CalledProcessError(1, command)

        self.cargo.side_effect = failing_clean
        with self.assertRaises(subprocess.CalledProcessError):
            cleanup.clean_build(self.target, "release")

        self.assert_binaries_preserved("release", contents)
        self.assert_unrelated_preserved()
        self.assertFalse(list(self.root.glob(".naru-build-keep-*")))

    def test_refuses_outside_target_without_running_cargo(self):
        outside = self.sandbox / "other-project-target"
        contents = self.binaries(outside / "release", "outside")

        with self.assertRaisesRegex(ValueError, "outside this checkout"):
            cleanup.clean_build(outside, "release")

        self.cargo.assert_not_called()
        for name, content in contents.items():
            self.assertEqual((outside / "release" / name).read_bytes(), content)
        self.assert_unrelated_preserved()

    def test_refuses_symlink_target_without_touching_destination(self):
        outside = self.sandbox / "other-project-target"
        contents = self.binaries(outside / "release", "outside")
        self.target.symlink_to(outside, target_is_directory=True)

        with self.assertRaisesRegex(ValueError, "outside this checkout"):
            cleanup.clean_build(self.target, "release")

        self.cargo.assert_not_called()
        self.assertTrue(self.target.is_symlink())
        for name, content in contents.items():
            self.assertEqual((outside / "release" / name).read_bytes(), content)
        self.assert_unrelated_preserved()

    def test_refuses_symlink_profile_without_running_cargo(self):
        outside = self.sandbox / "other-profile"
        contents = self.binaries(outside, "outside")
        self.target.mkdir()
        (self.target / "release").symlink_to(outside, target_is_directory=True)

        with self.assertRaisesRegex(ValueError, "symlink build profile"):
            cleanup.clean_build(self.target, "release")

        self.cargo.assert_not_called()
        for name, content in contents.items():
            self.assertEqual((outside / name).read_bytes(), content)
        self.assert_unrelated_preserved()

    def test_refuses_symlink_binary_without_running_cargo(self):
        self.binaries(self.target / "release")
        binary = self.target / "release" / cleanup.BINARIES[0]
        binary.unlink()
        source = self.root / "src/main.rs"
        binary.symlink_to(source)

        with self.assertRaisesRegex(ValueError, "Missing regular build executable"):
            cleanup.clean_build(self.target, "release")

        self.cargo.assert_not_called()
        self.assert_unrelated_preserved()


if __name__ == "__main__":
    unittest.main()
