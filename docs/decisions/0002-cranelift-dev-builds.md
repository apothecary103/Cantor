# 0002 Cranelift for dev builds, LLVM for release

Where: `.cargo/config.toml` sets `[unstable] codegen-backend = true`,
`profile.dev.codegen-backend = "cranelift"` and
`profile.release.codegen-backend = "llvm"`. It cannot go in Cargo.toml: that
needs `cargo-features = ["codegen-backend"]`, which makes stable cargo refuse
the whole workspace.

Assertion: `cargo xtask codegen-check` (part of `cargo xtask ci`) cleans and
rebuilds a dependency-free probe crate (cantor-plugin-sdk) in a separate
target dir with `--verbose`, and checks that rustc ran with
`-Zcodegen-backend=cranelift` in dev and `=llvm` in release. Checking the
flags cargo passes is the most direct observable; the binaries carry no
reliable marker of their backend.

Consequences:
- Stable cargo rejects the config file ("feature `codegen-backend` is
  required"). The stable check of cantor-plugin-sdk therefore runs from a
  directory outside the repo with `--manifest-path`, so cargo never reads
  `.cargo/config.toml`. Alternative considered: setting the backend through
  environment variables in the dev shell; rejected because plain `cargo`
  outside direnv would then silently build with LLVM.
- Miri and ThreadSanitizer override the dev backend to LLVM
  (`CARGO_PROFILE_DEV_CODEGEN_BACKEND=llvm`): Miri does not codegen, and
  sanitizers are LLVM-only.
- On aarch64-darwin, Cranelift emits return-address-signing CFI that ld64
  cannot encode as compact unwind; ld falls back to DWARF unwind info and
  prints a line per symbol. Unwinding works (proptest's panic catching relies
  on it), so the workspace allows the `linker_messages` lint.
- Dependencies build with LLVM even in dev (`[profile.dev.package."*"]`).
  On x86_64 Linux, Cranelift aborted at runtime on an SSE2 intrinsic
  (`llvm.x86.sse2.pmulhu.w`) used by a dependency. Workspace crates, where
  edit-compile cycles happen, keep Cranelift; the assertion checks a
  workspace crate.
