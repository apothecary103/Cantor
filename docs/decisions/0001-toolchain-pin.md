# 0001 Toolchain: nightly-2026-10-20

The pinned nightly is `nightly-2026-10-20` (rustc 1.101.0-nightly 07e526db9).
It was the newest nightly whose manifest had every component we need
(rustc, cargo, rust-std, rust-src, rustfmt, clippy, miri,
rustc-codegen-cranelift-preview) on all four flake systems
(aarch64/x86_64 × darwin/linux). Components are listed in
rust-toolchain.toml so fenix fetches exactly those; `rust-src` is required by
Miri and by `-Zbuild-std` for ThreadSanitizer.

The fenix hash in flake.nix matches this date. Bumping follows
docs/spec/10-stack.md ("Toolchain bumps").

The stable shell (`nix develop .#stable`) uses fenix's `stable` channel, which
moves with flake.lock. That is deliberate: the spec asks for "the latest
stable Rust", and updating flake.lock is already its own change.
