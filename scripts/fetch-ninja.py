#!/usr/bin/env python3
"""Install the verified official Ninja macOS binary into the workspace cache."""

import argparse
import hashlib
from pathlib import Path
import platform
import subprocess
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parent.parent
VERSION = "1.13.2"
URL = "https://github.com/ninja-build/ninja/releases/download/v1.13.2/ninja-mac.zip"
# Official GitHub release asset digest, checked against release v1.13.2 metadata:
# https://api.github.com/repos/ninja-build/ninja/releases/assets/318863524
SHA256 = "c99048673aa765960a99cf10c6ddb9f1fad506099ff0a0e137ad8960a88f321b"
ARCHIVE_SIZE = 314051


def verified(path):
    return (path.is_file() and path.stat().st_size == ARCHIVE_SIZE
            and hashlib.sha256(path.read_bytes()).hexdigest() == SHA256)


def fetch(cache_dir):
    if platform.system() != "Darwin":
        raise ValueError("The pinned Ninja archive requires macOS")
    cache_dir.mkdir(parents=True, exist_ok=True)
    archive = cache_dir / "ninja-mac.zip"
    if not verified(archive):
        partial = cache_dir / "ninja-mac.zip.part"
        subprocess.run(["/usr/bin/curl", "--fail", "--show-error", "--location",
                        "--proto", "=https", "--proto-redir", "=https",
                        "--connect-timeout", "30", "--retry", "2",
                        "--output", str(partial), URL], check=True)
        if not verified(partial):
            raise ValueError("Ninja archive does not match the official size and SHA-256 digest")
        partial.replace(archive)

    binary_dir = cache_dir / "bin"
    binary_dir.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive) as handle:
        # Extract only the named executable, never arbitrary archive paths.
        binary = handle.read("ninja")
    with tempfile.TemporaryDirectory(prefix=".ninja-", dir=binary_dir) as temporary:
        executable = Path(temporary) / "ninja"
        executable.write_bytes(binary)
        executable.chmod(0o755)
        version = subprocess.run([str(executable), "--version"], check=True,
                                 capture_output=True, text=True).stdout.strip()
        if version != VERSION:
            raise ValueError("Unexpected Ninja version after extraction: " + version)
        destination = binary_dir / "ninja"
        executable.replace(destination)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, default=ROOT / ".cache")
    args = parser.parse_args()
    try:
        destination = fetch(args.cache_dir.resolve())
    except (OSError, ValueError, KeyError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        parser.exit(1, "Ninja fetch failed: %s\n" % error)
    print(destination)


if __name__ == "__main__":
    main()
