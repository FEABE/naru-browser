#!/usr/bin/env python3
"""Fetch the pinned CEF runtime using macOS curl, preserving TLS verification.

This reproduces download-cef's macOS extraction layout for environments where
the Rust HTTP client's TLS handshake fails. No global installation is changed.
"""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile

from cef_config import CEF_CHANNEL, CEF_VERSION


ROOT = Path(__file__).resolve().parent.parent
CDN = "https://cef-builds.spotifycdn.com"
FRAMEWORK_BINARY = "Chromium Embedded Framework.framework/Chromium Embedded Framework"
BUILD_FILES = ("CMakeLists.txt", "cmake", "include", "libcef_dll", "CREDITS.html")


def curl(url, output=None):
    command = ["/usr/bin/curl", "--fail", "--show-error", "--location",
               "--proto", "=https", "--proto-redir", "=https",
               "--connect-timeout", "30", "--retry", "2"]
    if output is None:
        command.append("--silent")
    else:
        command.extend(["--output", str(output)])
    command.append(url)
    return subprocess.run(command, check=True, stdout=subprocess.PIPE if output is None else None).stdout


def matches_archive(path, metadata):
    if not path.is_file() or path.stat().st_size != metadata["size"]:
        return False
    digest = hashlib.sha1()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest() == metadata["sha1"]


def cached_cef(cache_dir, index_key, rust_arch):
    candidates = [cache_dir / CEF_VERSION / ("cef_macos_" + rust_arch), cache_dir]
    for candidate in candidates:
        try:
            metadata = json.loads((candidate / "archive.json").read_text())
            if not isinstance(metadata, dict):
                continue
            archive_name = metadata.get("name")
            sha1 = metadata.get("sha1")
            if (metadata.get("type") == "minimal"
                    and isinstance(archive_name, str) and isinstance(sha1, str)
                    and archive_name.startswith("cef_binary_" + CEF_VERSION + "+")
                    and archive_name.endswith("_" + index_key + "_" + CEF_CHANNEL + "_minimal.tar.bz2")
                    and re.fullmatch(r"[a-f0-9]{40}", sha1)
                    and (candidate / FRAMEWORK_BINARY).is_file()
                    and all((candidate / name).exists() for name in BUILD_FILES)):
                return candidate
        except (OSError, ValueError):
            continue
    return None


def fetch(cache_dir):
    systems = {"arm64": ("macosarm64", "aarch64"), "x86_64": ("macosx64", "x86_64")}
    if platform.system() != "Darwin" or platform.machine() not in systems:
        raise ValueError("This script requires macOS arm64 or x86_64")
    index_key, rust_arch = systems[platform.machine()]
    cached = cached_cef(cache_dir, index_key, rust_arch)
    if cached is not None:
        return cached
    index = json.loads(curl(CDN + "/index.json"))
    versions = [entry for entry in index[index_key]["versions"]
                if entry["cef_version"].startswith(CEF_VERSION + "+")]
    if len(versions) != 1:
        raise ValueError("Expected one CEF " + CEF_VERSION + " entry in the official CDN index")
    files = [entry for entry in versions[0]["files"] if entry["type"] == "minimal"]
    if len(files) != 1:
        raise ValueError("Expected one minimal archive in the official CDN index")
    metadata = files[0]
    name = metadata["name"]
    if (not re.fullmatch(r"[A-Za-z0-9_.+\-]+\.tar\.bz2", name)
            or not re.fullmatch(r"[a-f0-9]{40}", metadata["sha1"])
            or not isinstance(metadata["size"], int) or metadata["size"] <= 0):
        raise ValueError("Invalid CEF archive metadata")
    print(json.dumps({"url": CDN + "/" + name, "size": metadata["size"],
                      "sha1": metadata["sha1"]}, indent=2), flush=True)
    version_dir = cache_dir / CEF_VERSION
    version_dir.mkdir(parents=True, exist_ok=True)
    destination = version_dir / ("cef_macos_" + rust_arch)
    archive_info = {key: metadata[key] for key in ("type", "name", "sha1")}
    if destination.exists():
        archive_json = destination / "archive.json"
        if (archive_json.is_file() and json.loads(archive_json.read_text()) == archive_info
                and (destination / FRAMEWORK_BINARY).is_file()
                and all((destination / name).exists() for name in BUILD_FILES)):
            return destination
        raise ValueError("Existing incomplete or different CEF directory: " + str(destination))

    archive = version_dir / name
    if not matches_archive(archive, metadata):
        partial = version_dir / (name + ".part")
        if not matches_archive(partial, metadata):
            curl(CDN + "/" + name, partial)
        if not matches_archive(partial, metadata):
            raise ValueError("Downloaded CEF archive size or SHA-1 does not match the official index")
        partial.replace(archive)
    print("Archive size and SHA-1 verified. Extracting...", flush=True)

    with tempfile.TemporaryDirectory(prefix=".extract-", dir=version_dir) as temporary:
        # macOS bsdtar rejects absolute/parent paths and unsafe symlink traversal
        # by default. Do not add -P (which disables those protections).
        subprocess.run(["/usr/bin/tar", "-xjf", str(archive), "-C", temporary], check=True)
        extracted = Path(temporary) / name.removesuffix(".tar.bz2")
        release = extracted / "Release"
        if not (release / FRAMEWORK_BINARY).is_file():
            raise ValueError("Archive is missing its CEF framework")
        for name in BUILD_FILES:
            (extracted / name).rename(release / name)
        # Keep the upstream license as well; cef-rs itself drops this during extraction.
        if (extracted / "LICENSE.txt").is_file():
            (extracted / "LICENSE.txt").rename(release / "LICENSE.txt")
        (release / "archive.json").write_text(json.dumps(archive_info, indent=2) + "\n")
        release.rename(destination)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, default=ROOT / ".cache/cef")
    args = parser.parse_args()
    try:
        destination = fetch(args.cache_dir.resolve())
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, "CEF fetch failed: %s\n" % error)
    print(destination)


if __name__ == "__main__":
    main()
