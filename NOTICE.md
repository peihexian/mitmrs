# Third-party notices

## WinDivert 2.2.2-A

This repository bundles WinDivert 2.2.2-A under `third_part/WinDivert-2.2.2-A`.
WinDivert is Copyright (c) 2012-2023 Basil (reqrypt.org) and is dual-licensed
under the GNU Lesser General Public License v3 (LGPLv3) or the GNU General
Public License v2 (GPLv2). See `third_part/WinDivert-2.2.2-A/LICENSE` for the
full text. Redistributing `WinDivert.dll` and `WinDivert64.sys` must comply with
that license and any code-signing requirements.

## Rust crates

The Rust dependency tree is recorded in `Cargo.lock`. Each crate retains its own
license; `cargo deny` or `cargo about` can be used to produce a complete report.
Notable dependencies include axum, hyper, tokio, rustls, rcgen, rusqlite,
rust-embed and the `windivert` bindings.
