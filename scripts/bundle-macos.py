#!/usr/bin/env python3
"""Bundle the Rust executables with CEF for local macOS development.

Layout follows cef-rs/cef/src/build_util/mac.rs. LibraryLoader loads the
framework relative to the executable, so no rpath changes are necessary.
Signing defaults to ad hoc for local use; this does not create a notarized release.
Pass --signing-identity to use an explicitly selected code-signing identity.
"""

import argparse
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import time

from cef_config import CEF_VERSION


ROOT = Path(__file__).resolve().parent.parent
APP_NAME = "Light Browser"
APP_ID = "local.lightbrowser.dev"
MAIN_BINARY = "light-browser"
HELPER_BINARY = "light-browser-helper"
CLI_BINARY = "lightbrowser"
FRAMEWORK = "Chromium Embedded Framework.framework"
HELPERS = (("", ""), (" (GPU)", ".gpu"), (" (Renderer)", ".renderer"),
           (" (Plugin)", ".plugin"), (" (Alerts)", ".alerts"))


def validate_framework_version(framework):
    # Accept both the flattened download-cef cache and an original CEF archive.
    distribution = framework.parent
    if distribution.name == "Release" and not (distribution / "include").is_dir():
        distribution = distribution.parent
    header = distribution / "include/cef_version.h"
    try:
        text = header.read_text(encoding="utf-8")
    except OSError as error:
        raise ValueError("Cannot verify CEF version; missing readable header: " + str(header)) from error
    versions = re.findall(r'^\s*#define\s+CEF_VERSION\s+"([^"\r\n]+)"\s*$', text, re.MULTILINE)
    if len(versions) != 1 or versions[0].split("+", 1)[0] != CEF_VERSION:
        raise ValueError("Expected CEF %s in %s, found %s" % (CEF_VERSION, header, versions))

    archive = distribution / "archive.json"
    if archive.exists():
        try:
            metadata = json.loads(archive.read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            raise ValueError("Cannot verify CEF archive metadata: " + str(archive)) from error
        name = metadata.get("name") if isinstance(metadata, dict) else None
        if not isinstance(name, str) or not name.startswith("cef_binary_" + versions[0] + "_"):
            raise ValueError("CEF archive metadata does not match its version header: " + str(archive))

    # Check the selected framework too, so a different runtime next to a valid
    # header is rejected before the output directory or existing app is touched.
    info_path = framework / "Resources/Info.plist"
    try:
        with info_path.open("rb") as handle:
            info = plistlib.load(handle)
    except (OSError, ValueError, plistlib.InvalidFileException) as error:
        raise ValueError("Cannot verify CEF framework metadata: " + str(info_path)) from error
    version = info.get("CFBundleShortVersionString") if isinstance(info, dict) else None
    if version != CEF_VERSION + ".0":
        raise ValueError("Expected CEF framework %s.0 in %s, found %r" %
                         (CEF_VERSION, info_path, version))


def find_framework(cef_path):
    if cef_path.name == FRAMEWORK:
        candidates = [cef_path]
    else:
        arch = {"arm64": "aarch64", "x86_64": "x86_64"}.get(platform.machine())
        if arch is None:
            raise ValueError("Unsupported macOS architecture: " + platform.machine())
        platform_dir = "cef_macos_" + arch
        candidates = [cef_path / FRAMEWORK, cef_path / "Release" / FRAMEWORK,
                      cef_path / platform_dir / FRAMEWORK]
        candidates.append(cef_path / CEF_VERSION / platform_dir / FRAMEWORK)
    matches = sorted({path.resolve() for path in candidates
                      if (path / "Chromium Embedded Framework").is_file()})
    if len(matches) != 1:
        raise ValueError(
            "Expected one CEF framework under %s, found %d. "
            "Set CEF_PATH to the exact downloaded CEF directory." % (cef_path, len(matches))
        )
    validate_framework_version(matches[0])
    return matches[0]


def create_app(path, source_binary, executable_name, identifier, display_name, helper=False):
    contents = path / "Contents"
    for directory in ("MacOS", "Resources", "Frameworks"):
        (contents / directory).mkdir(parents=True)
    shutil.copy2(source_binary, contents / "MacOS" / executable_name)
    info = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleDisplayName": display_name,
        "CFBundleExecutable": executable_name,
        "CFBundleIdentifier": identifier,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": display_name,
        "CFBundlePackageType": "APPL",
        "CFBundleSignature": "????",
        "CFBundleVersion": "0.1.0",
        "CFBundleShortVersionString": "0.1.0",
        "LSEnvironment": {"MallocNanoZone": "0"},
        "LSFileQuarantineEnabled": True,
        "LSMinimumSystemVersion": "13.0",
        "NSHighResolutionCapable": True,
        "NSSupportsAutomaticGraphicsSwitching": True,
        "NSCameraUsageDescription": "Allow websites to use the camera with your permission.",
        "NSMicrophoneUsageDescription": "Allow websites to use the microphone with your permission.",
        "NSLocationUsageDescription": "Allow websites to request your location with your permission.",
        "NSBluetoothAlwaysUsageDescription": "Allow websites to use Bluetooth with your permission.",
        "NSWebBrowserPublicKeyCredentialUsageDescription": "Use passkeys to sign in to websites.",
    }
    if helper:
        info["LSUIElement"] = True
    # The browser initializes its CEF-compatible NSApplication subclass itself.
    with (contents / "Info.plist").open("wb") as handle:
        plistlib.dump(info, handle)
    (contents / "PkgInfo").write_bytes(b"APPL????")


def signing_identity(value):
    if not value.strip():
        raise argparse.ArgumentTypeError("Signing identity must not be empty or whitespace-only.")
    return value


def sign(path, identity):
    # A selected identity must fail explicitly; never fall back to ad hoc.
    subprocess.run(["/usr/bin/codesign", "--force", "--sign", identity, str(path)], check=True)


def is_macho(path):
    if path.is_symlink() or not path.is_file():
        return False
    with path.open("rb") as handle:
        return handle.read(4) in (
            b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe",
            b"\xfe\xed\xfa\xcf", b"\xcf\xfa\xed\xfe",
            b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",
            b"\xca\xfe\xba\xbf", b"\xbf\xba\xfe\xca",
        )


def stop_previous_app(destination):
    # NSRunningApplication targets a PID and requests normal Cocoa termination.
    # Match its executable path too: another checkout can share our bundle ID.
    executable = str(destination / "Contents/MacOS" / MAIN_BINARY)
    script = r'''
ObjC.import('AppKit');
function run(argv) {
    const apps = $.NSRunningApplication.runningApplicationsWithBundleIdentifier(argv[0]);
    const matched = [];
    for (let i = 0; i < apps.count; i++) {
        const app = apps.objectAtIndex(i);
        if (ObjC.unwrap(app.executableURL.path) === argv[1]) {
            if (!app.terminate) throw new Error('The previous app refused to quit.');
            matched.push(app.processIdentifier);
        }
    }
    return JSON.stringify(matched);
}
'''
    result = subprocess.run(["/usr/bin/osascript", "-l", "JavaScript", "-", APP_ID, executable],
                            input=script, text=True, capture_output=True, timeout=20)
    if result.returncode:
        raise ValueError("Cannot safely stop the previous app: " + result.stderr.strip())
    pids = json.loads(result.stdout)
    deadline = time.monotonic() + 15
    for pid in pids:
        while True:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            if time.monotonic() >= deadline:
                raise ValueError("The previous app is still running; close its windows first.")
            time.sleep(0.1)
    if pids:
        print("Stopped previous app before replacing its bundle.", file=sys.stderr)


def bundle(target_dir, cef_path, output_dir, identity="-"):
    for name in (MAIN_BINARY, HELPER_BINARY, CLI_BINARY):
        if not (target_dir / name).is_file():
            raise ValueError("Missing executable: %s. Run cargo build --bins first." % (target_dir / name))
    framework_source = find_framework(cef_path)
    output_dir.mkdir(parents=True, exist_ok=True)
    destination = output_dir / (APP_NAME + ".app")
    if destination.exists():
        with (destination / "Contents/Info.plist").open("rb") as handle:
            existing_id = plistlib.load(handle).get("CFBundleIdentifier")
        if existing_id != APP_ID or destination.is_symlink():
            raise ValueError("Refusing to replace an unrelated app: " + str(destination))

    with tempfile.TemporaryDirectory(prefix=".light-browser-build-", dir=output_dir) as temporary:
        app = Path(temporary) / (APP_NAME + ".app")
        create_app(app, target_dir / MAIN_BINARY, MAIN_BINARY, APP_ID, APP_NAME)
        frameworks = app / "Contents/Frameworks"
        framework = frameworks / FRAMEWORK
        shutil.copytree(framework_source, framework, symlinks=True)

        # Sign nested code first, without applying executable entitlements to libraries.
        framework_binary = (framework / "Chromium Embedded Framework").resolve()
        for candidate in sorted(framework.rglob("*")):
            if candidate.resolve() != framework_binary and is_macho(candidate):
                sign(candidate, identity)
        sign(framework, identity)

        for suffix, id_suffix in HELPERS:
            # CEF derives helper paths from the main executable's basename.
            name = MAIN_BINARY + " Helper" + suffix
            helper = frameworks / (name + ".app")
            create_app(helper, target_dir / HELPER_BINARY, name,
                       APP_ID + ".helper" + id_suffix, APP_NAME + " Helper" + suffix, True)
            sign(helper, identity)

        cli = app / "Contents/MacOS" / CLI_BINARY
        shutil.copy2(target_dir / CLI_BINARY, cli)
        sign(cli, identity)
        shutil.copy2(ROOT / "THIRD_PARTY_NOTICES.md", app / "Contents/Resources/THIRD_PARTY_NOTICES.md")
        credits = framework_source.parent / "CREDITS.html"
        if credits.is_file():
            shutil.copy2(credits, app / "Contents/Resources/CEF-CREDITS.html")
        license_file = framework_source.parent / "LICENSE.txt"
        if license_file.is_file():
            shutil.copy2(license_file, app / "Contents/Resources/CEF-LICENSE.txt")
        sign(app, identity)
        subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        stop_previous_app(destination)
        # Retain the previous generated bundle until the replacement has verified.
        previous = Path(temporary) / "previous.app"
        if destination.exists():
            destination.rename(previous)
        try:
            app.rename(destination)
        except OSError:
            if previous.exists():
                previous.rename(destination)
            raise
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target/debug")
    parser.add_argument("--cef-path", type=Path, default=Path(os.environ.get("CEF_PATH", ROOT / ".cache/cef")))
    parser.add_argument("--output-dir", type=Path, default=ROOT / "dist")
    parser.add_argument("--signing-identity", type=signing_identity, default="-",
                        help="Code-signing identity to use for all nested code and the app (default: ad hoc).")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("macOS is required to bundle and sign this app")
    try:
        result = bundle(args.target_dir.resolve(), args.cef_path.resolve(),
                        args.output_dir.resolve(), args.signing_identity)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, "Bundling failed: %s\n" % error)
    print(result)


if __name__ == "__main__":
    main()
