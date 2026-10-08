# Local macOS CEF bindings

Source: https://github.com/tauri-apps/cef-rs at commit
`2861f6991fae3ee31f877891382adcbc9b9f0953`.

This is a local derivative, not a published cef-rs release. The `cef` and
`cef-dll-sys` package versions are `156.0.0+156.0.1` to identify the paired
CEF runtime: `156.0.1+g8db2f36+chromium-156.0.8078.17` (beta).

Changes from upstream:

- Regenerated both macOS architectures' raw bindings, safe bindings and resource
  constants using the upstream `update-bindings` tool and official CEF headers.
- Explicit `BINDGEN_EXTRA_CLANG_ARGS=-DCEF_API_VERSION=15600`. The C++ wrapper
  build also selects `CEF_API_VERSION_LAST=15600`. Experimental ABI is not used.
- Kept only the macOS platform modules. Windows/Linux are not supported by this copy.
- Restricted workspace members to `cef` and `sys`, updated their version and
  dependency, and used published `download-cef=3.0` for the existing build script.
- No CEF framework patch, encryption override, or modified Keychain access policy.

Regeneration, from an isolated checkout of the pinned upstream source:

```bash
CEF_PATH_macos_aarch64="/absolute/path/to/cef-with-include" \
BINDGEN_EXTRA_CLANG_ARGS='-DCEF_API_VERSION=15600' \
cargo run -p update-bindings -- --bindgen --target aarch64-apple-darwin

CEF_PATH_macos_x86_64="/absolute/path/to/cef-with-include" \
BINDGEN_EXTRA_CLANG_ARGS='-DCEF_API_VERSION=15600' \
cargo run -p update-bindings -- --bindgen --target x86_64-apple-darwin
```

The generator version was bindgen 0.73.2. Both platform bindings were generated
against the same official macOS headers; runtime execution is tested on ARM64.
The application checks the loaded API hash before initializing CEF:
`e01e9208f887490aa249d57f56761ab9c0c3f7ac`.

Runtime archive source: https://cef-builds.spotifycdn.com/index.json .
macOS ARM64 minimal archive: 134822382 bytes,
SHA-1 `00027a60d5e7b0fed571e0179462056aed126f38`.
