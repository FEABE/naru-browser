#!/usr/bin/env python3
"""Remove this checkout's Cargo intermediates, retaining its latest executables."""

import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent.parent
BINARIES = ("light-browser", "light-browser-helper", "lightbrowser")


def clean_build(target, profile):
    local_target = ROOT / "target"
    if target.is_symlink() or target.resolve() != local_target:
        raise ValueError("Refusing to clean a target outside this checkout: " + str(target))
    selected = target / profile
    if selected.is_symlink():
        raise ValueError("Refusing a symlink build profile: " + str(selected))
    for name in BINARIES:
        if not (selected / name).is_file() or (selected / name).is_symlink():
            raise ValueError("Missing regular build executable: " + str(selected / name))

    # Cargo owns its build locks and removal rules. Keep the CLI paths usable
    # after cleaning both profiles, and restore them even if clean fails midway.
    with tempfile.TemporaryDirectory(prefix=".naru-build-keep-", dir=ROOT) as temporary:
        saved = Path(temporary)
        for name in BINARIES:
            shutil.copy2(selected / name, saved / name)
        try:
            subprocess.run(["cargo", "clean", "--manifest-path", str(ROOT / "Cargo.toml"),
                            "--target-dir", str(local_target)], check=True, cwd=ROOT)
        finally:
            if local_target.is_symlink() or selected.is_symlink():
                raise ValueError("Build target changed while cleaning; refusing to restore through a symlink")
            selected.mkdir(parents=True, exist_ok=True)
            for name in BINARIES:
                shutil.copy2(saved / name, selected / name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target")
    parser.add_argument("--profile", choices=("debug", "release"), required=True)
    args = parser.parse_args()
    try:
        clean_build(args.target_dir.absolute(), args.profile)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, "Build cleanup failed: %s\n" % error)
    print("Removed Cargo intermediates; retained latest %s executables." % args.profile)


if __name__ == "__main__":
    main()
