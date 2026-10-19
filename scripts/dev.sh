#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Darwin" ]]; then
  printf '%s\n' 'Light Browser currently requires macOS.' >&2
  exit 1
fi

build_profile=debug
run_app=0
build_only_requested=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) build_profile=release; shift ;;
    --build-only) build_only_requested=1; shift ;;
    --run) run_app=1; shift ;;
    --) shift; break ;;
    -h|--help)
      printf '%s\n' 'Usage: scripts/dev.sh [--release] [--build-only | --run] [-- browser arguments]'
      printf '%s\n' 'Builds without launching by default. --run requires an explicitly selected non-ad-hoc NARU_CODESIGN_IDENTITY,'
      printf '%s\n' 'or, for verification only, NARU_MOCK_KEYCHAIN=1 with LIGHT_BROWSER_DATA_DIR set (mock keychain, separate profile).'
      printf '%s\n' 'Build-only defaults to ad hoc signing. A selected identity does not guarantee Keychain authorization.'
      exit 0
      ;;
    *) printf 'Unknown option: %s. Put browser arguments after -- and select --run.\n' "$1" >&2; exit 2 ;;
  esac
done

if [[ "$run_app" == 1 && "$build_only_requested" == 1 ]]; then
  printf '%s\n' '--run and --build-only cannot be combined.' >&2
  exit 2
fi
if [[ "$run_app" == 0 && $# -gt 0 ]]; then
  printf '%s\n' 'Browser arguments require --run; no build or launch was started.' >&2
  exit 2
fi

bundle_args=(scripts/bundle-macos.py)
if [[ "${NARU_CODESIGN_IDENTITY+x}" == x ]]; then
  if [[ ! "$NARU_CODESIGN_IDENTITY" =~ [^[:space:]] ]]; then
    printf '%s\n' 'NARU_CODESIGN_IDENTITY must not be empty or whitespace-only.' >&2
    exit 1
  fi
  bundle_args+=(--signing-identity "$NARU_CODESIGN_IDENTITY")
fi
mock_keychain=0
if [[ "${NARU_MOCK_KEYCHAIN:-}" == 1 && -n "${LIGHT_BROWSER_DATA_DIR:-}" ]]; then
  mock_keychain=1
fi
if [[ "$run_app" == 1 && "${NARU_CODESIGN_IDENTITY:--}" == '-' && "$mock_keychain" == 0 ]]; then
  printf '%s\n' 'Automatic launch of an ad-hoc build is disabled because changed builds can request Keychain access again.' >&2
  printf '%s\n' 'Use --build-only, or select an existing signing identity with NARU_CODESIGN_IDENTITY before --run.' >&2
  printf '%s\n' 'For verification only, set NARU_MOCK_KEYCHAIN=1 and LIGHT_BROWSER_DATA_DIR to use a mock keychain.' >&2
  exit 2
fi

# Keep build downloads and this prototype's browsing data inside the workspace.
export CEF_PATH="${CEF_PATH:-$repo_root/.cache/cef}"
export LIGHT_BROWSER_DATA_DIR="${LIGHT_BROWSER_DATA_DIR:-$repo_root/.local-data}"
export MallocNanoZone=0
export PATH="$repo_root/.cache/bin:$PATH"

# The fetcher returns without network access when this CEF version is cached.
python3 scripts/fetch-cef.py --cache-dir "$CEF_PATH"
if ! command -v ninja >/dev/null 2>&1; then
  python3 scripts/fetch-ninja.py
fi

if [[ "$build_profile" == release ]]; then
  cargo build --locked --bins --release
else
  cargo build --locked --bins
fi
target_root="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
python3 "${bundle_args[@]}" --target-dir "$target_root/$build_profile" --cef-path "$CEF_PATH"
python3 scripts/cleanup-build.py --target-dir "$target_root" --profile "$build_profile"

if [[ "$run_app" == 1 ]]; then
  exec "$repo_root/dist/Light Browser.app/Contents/MacOS/light-browser" "$@"
fi
printf '%s\n' 'Build complete. The app was not launched.'
