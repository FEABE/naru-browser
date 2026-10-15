"""Exercise dev.sh with an isolated PATH; no real build, signing, or app launch."""

import json
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "dev.sh"
IDENTITY = "Developer ID Application: Naru Test (TEAM 123)"

# These stubs do not forward commands or evaluate the Python supplied to -c.
# In particular, the pre-created fake app records an accidental launch even if
# a previous build stage failed. PATH contains no system command directories.
STUB = r'''
import json
import os
from pathlib import Path
import sys

command, *args = sys.argv[1:]
if command == "dirname":
    print(str(Path(args[0]).parent))
    sys.exit(0)
if command == "uname":
    assert args == ["-s"]
    print("Darwin")
    sys.exit(0)

if command == "python3":
    stages = {
        "scripts/fetch-cef.py": "fetch_cef",
        "scripts/fetch-ninja.py": "fetch_ninja",
        "scripts/bundle-macos.py": "bundle",
        "scripts/cleanup-build.py": "cleanup",
        "-c": "metadata_parse",
    }
    stage = stages.get(args[0] if args else "")
elif command == "cargo":
    stage = {"build": "build", "metadata": "metadata"}.get(args[0] if args else "")
elif command == "app":
    stage = "app"
else:
    stage = None
if stage is None:
    print("Unexpected stub command", file=sys.stderr)
    sys.exit(90)

record = {"stage": stage, "argv": args,
          "dataDir": os.environ.get("LIGHT_BROWSER_DATA_DIR"),
          "cefPath": os.environ.get("CEF_PATH")}
fd = os.open(os.environ["DEV_TEST_LOG"], os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
try:
    os.write(fd, (json.dumps(record) + "\n").encode())
finally:
    os.close(fd)
if os.environ.get("DEV_TEST_FAIL") == stage:
    sys.exit(23)
if stage == "metadata":
    print(json.dumps({"target_directory": os.environ["DEV_TEST_TARGET"]}))
elif stage == "metadata_parse":
    try:
        print(json.load(sys.stdin)["target_directory"])
    except (ValueError, KeyError):
        sys.exit(24)
'''


class DevScriptTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="naru-dev-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve() / "checkout with spaces"
        scripts = self.root / "scripts"
        scripts.mkdir(parents=True)
        self.script = scripts / "dev.sh"
        shutil.copyfile(SCRIPT, self.script)
        self.bin = self.root / ".cache" / "bin"
        self.bin.mkdir(parents=True)
        self.log = self.root / "calls.jsonl"
        self.target = self.root / "custom target"
        self.stub = self.root / "stub.py"
        self.stub.write_text(STUB)
        for name in ("dirname", "uname", "python3", "cargo", "ninja"):
            self.write_command(self.bin / name, name)
        self.app = self.root / "dist" / "Light Browser.app" / "Contents" / "MacOS" / "light-browser"
        self.app.parent.mkdir(parents=True)
        self.write_command(self.app, "app")
        (self.root / "home").mkdir()
        (self.root / "tmp").mkdir()

    def write_command(self, path, name):
        invocation = " ".join(shlex.quote(value) for value in (sys.executable, str(self.stub), name))
        path.write_text('#!/bin/sh\nexec ' + invocation + ' "$@"\n')
        path.chmod(0o700)

    def run_dev(self, *arguments, identity=None, fail=None):
        self.log.unlink(missing_ok=True)
        # Do not inherit signing choices, shell hooks, or the user's PATH/profile.
        environment = {
            "PATH": str(self.bin), "HOME": str(self.root / "home"),
            "TMPDIR": str(self.root / "tmp"), "LC_ALL": "C",
            "DEV_TEST_LOG": str(self.log), "DEV_TEST_TARGET": str(self.target),
        }
        if identity is not None:
            environment["NARU_CODESIGN_IDENTITY"] = identity
        if fail is not None:
            environment["DEV_TEST_FAIL"] = fail
        result = subprocess.run(
            ["/bin/bash", str(self.script), *arguments], cwd=self.root,
            env=environment, text=True, capture_output=True, timeout=10,
        )
        calls = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return result, calls

    def stages(self, calls):
        # The metadata producer and consumer run concurrently in a shell pipe.
        return [call["stage"] for call in calls if call["stage"] != "metadata_parse"]

    def call(self, calls, stage):
        matches = [call for call in calls if call["stage"] == stage]
        self.assertEqual(len(matches), 1, calls)
        return matches[0]

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_default_debug_and_release_build_without_launching(self):
        for arguments, profile in (((), "debug"), (("--release",), "release")):
            with self.subTest(profile=profile):
                result, calls = self.run_dev(*arguments)
                self.assert_success(result)
                self.assertEqual(self.stages(calls), ["fetch_cef", "build", "metadata", "bundle", "cleanup"])
                build = ["build", "--locked", "--bins"] + (["--release"] if profile == "release" else [])
                self.assertEqual(self.call(calls, "build")["argv"], build)
                self.assertEqual(self.call(calls, "bundle")["argv"], [
                    "scripts/bundle-macos.py", "--target-dir", str(self.target / profile),
                    "--cef-path", str(self.root / ".cache" / "cef"),
                ])
                self.assertEqual(self.call(calls, "cleanup")["argv"], [
                    "scripts/cleanup-build.py", "--target-dir", str(self.target), "--profile", profile,
                ])
                self.assertIn("not launched", result.stdout)

    def test_explicit_build_only_preserves_selected_identity(self):
        for identity in (None, "-", IDENTITY):
            with self.subTest(identity=identity):
                result, calls = self.run_dev("--build-only", identity=identity)
                self.assert_success(result)
                arguments = self.call(calls, "bundle")["argv"]
                if identity is None:
                    self.assertNotIn("--signing-identity", arguments)
                else:
                    index = arguments.index("--signing-identity")
                    self.assertEqual(arguments[index + 1], identity)
                self.assertNotIn("app", self.stages(calls))

    def test_run_requires_selected_identity_and_preserves_browser_arguments(self):
        arguments = ["--url", "https://example.invalid/a path", "literal $HOME $(not-a-command)"]
        result, calls = self.run_dev("--release", "--run", "--", *arguments, identity=IDENTITY)
        self.assert_success(result)
        self.assertEqual(self.stages(calls), ["fetch_cef", "build", "metadata", "bundle", "cleanup", "app"])
        bundle = self.call(calls, "bundle")["argv"]
        self.assertEqual(bundle[bundle.index("--signing-identity") + 1], IDENTITY)
        app = self.call(calls, "app")
        self.assertEqual(app["argv"], arguments)
        self.assertEqual(app["dataDir"], str(self.root / ".local-data"))
        self.assertEqual(app["cefPath"], str(self.root / ".cache" / "cef"))

    def test_run_rejects_missing_empty_whitespace_and_ad_hoc_identity_early(self):
        for identity in (None, "", " \t\n", "-"):
            with self.subTest(identity=identity):
                result, calls = self.run_dev("--run", identity=identity)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [])
                self.assertIn("NARU_CODESIGN_IDENTITY", result.stderr)

    def test_conflicting_flags_are_rejected_in_both_orders(self):
        for arguments in (("--run", "--build-only"), ("--build-only", "--run")):
            with self.subTest(arguments=arguments):
                result, calls = self.run_dev(*arguments, identity=IDENTITY)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [])
                self.assertIn("cannot be combined", result.stderr)

    def test_build_only_rejects_browser_arguments(self):
        for arguments in (("--", "https://example.invalid"), ("--build-only", "--", "an argument")):
            with self.subTest(arguments=arguments):
                result, calls = self.run_dev(*arguments, identity=IDENTITY)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [])
                self.assertIn("require --run", result.stderr)

    def test_failed_stage_prevents_following_stages_and_launch(self):
        stages = ["fetch_cef", "build", "metadata", "bundle", "cleanup"]
        for index, stage in enumerate(stages):
            with self.subTest(stage=stage):
                result, calls = self.run_dev("--run", identity=IDENTITY, fail=stage)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.stages(calls), stages[:index + 1])

    def test_failed_ninja_fetch_prevents_build_and_launch(self):
        (self.bin / "ninja").unlink()
        result, calls = self.run_dev("--run", identity=IDENTITY, fail="fetch_ninja")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.stages(calls), ["fetch_cef", "fetch_ninja"])

    def test_help_does_no_build_work(self):
        for argument in ("--help", "-h"):
            with self.subTest(argument=argument):
                result, calls = self.run_dev(argument)
                self.assert_success(result)
                self.assertEqual(calls, [])
                self.assertIn("--run", result.stdout)
                self.assertIn("without launching by default", result.stdout)


if __name__ == "__main__":
    unittest.main()
