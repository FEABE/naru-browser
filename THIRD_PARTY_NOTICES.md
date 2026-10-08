# Third-party notices

`src/mac.rs` follows the macOS application protocol integration in the cefsimple example from [tauri-apps/cef-rs](https://github.com/tauri-apps/cef-rs), inspected at commit `2861f6991fae3ee31f877891382adcbc9b9f0953`. The applicable MIT notice is reproduced below.

`vendor/cef-rs` contains the runtime crates from the same commit with macOS bindings regenerated from the official CEF 156.0.1 beta headers, API 15600. Local changes and reproduction instructions are recorded in `vendor/cef-rs/README.local.md`. The upstream MIT and Apache-2.0 license files are retained there.

MIT License

Copyright (c) 2023 Wu Yuwei

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

The app bundle includes the CEF license and Chromium third-party credits in its Resources directory. Rust dependency versions and package checksums are recorded in Cargo.lock. A production distribution should include the remaining Rust dependency license notices as well.
