#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Darwin" ]]; then
  printf '%s\n' 'Light Browser currently requires macOS.' >&2
  exit 1
fi

build_profile=debug
run_app=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) build_profile=release; shift ;;
    --build-only) run_app=0; shift ;;
    --) shift; break ;;
    -h|--help)
      printf '%s\n' 'Usage: scripts/dev.sh [--release] [--build-only] [-- browser arguments]'
      exit 0
      ;;
    *) break ;;
  esac
done

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
python3 scripts/bundle-macos.py --target-dir "$target_root/$build_profile" --cef-path "$CEF_PATH"

if [[ "$run_app" == 1 ]]; then
  exec "$repo_root/dist/Light Browser.app/Contents/MacOS/light-browser" "$@"
fi
