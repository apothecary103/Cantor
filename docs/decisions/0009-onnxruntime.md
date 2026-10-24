# 0009 ONNX Runtime: official 1.31.0 release from the flake

ort is pinned to `=2.0.0-rc.15`, built for ONNX Runtime 1.31 (API 31).
nixpkgs-unstable ships onnxruntime 1.27.1 and its macOS build has no CoreML
execution provider. As CLAUDE.md prescribes, flake.nix fetches the official
v1.31.0 release tarballs by sha256 (from the GitHub release digests) for
aarch64-darwin, x86_64-linux and aarch64-linux, patches the Linux library
with autoPatchelf, and exports ORT_DYLIB_PATH to it.

Microsoft publishes no x86_64 macOS build of 1.31. Intel Macs are best
effort, so x86_64-darwin falls back to nixpkgs' onnxruntime; ort then logs a
version-mismatch warning and CoreML is unavailable there.

CUDA/TensorRT builds (gpu tarballs, 230–430 MiB) are not fetched by default;
users with NVIDIA GPUs can point ORT_DYLIB_PATH at a GPU build.
