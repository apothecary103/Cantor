{
  description = "Cantor – singing voice synthesis workstation (dev environment)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ flake-parts, nixpkgs, fenix, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ];

      perSystem = { system, ... }:
        let
          pkgs = import nixpkgs { inherit system; };
          inherit (pkgs) lib stdenv;

          # Single source of truth: the pinned nightly in rust-toolchain.toml.
          # When the date in rust-toolchain.toml changes, set sha256 to
          # lib.fakeSha256, run `nix develop`, and paste the hash from the error.
          rust = fenix.packages.${system}.fromToolchainFile {
            file = ./rust-toolchain.toml;
            sha256 = "sha256-ZBbs8/vrgUCyEPZ+2MU1MQEIDLVI05q8t1JRuFFQytw=";
          };

          # Latest stable Rust (cantor-plugin-sdk, the phonemizer template,
          # and tools built from source).
          stableRust = fenix.packages.${system}.stable.withComponents [
            "cargo"
            "rustc"
            "rust-std"
          ];

          # Validates the DAW bridge plugin (`cargo xtask bridge --validate`).
          # Not in nixpkgs; built from the release tag.
          clap-validator =
            (pkgs.makeRustPlatform {
              cargo = stableRust;
              rustc = stableRust;
            }).buildRustPackage
              rec {
                pname = "clap-validator";
                version = "0.4.1";
                src = pkgs.fetchFromGitHub {
                  owner = "free-audio";
                  repo = "clap-validator";
                  rev = version;
                  hash = "sha256-4GmmZIMRoPUEbsT34iCaOWRhYmhMonF9BXnc/rFQV0M=";
                };
                cargoHash = "sha256-m6VebZM8jVm22Xk8URpHF+UAHOJWYC74Ha3bpFuz1VU=";
                doCheck = false;
              };

          # Libraries GPUI and cpal load at runtime on Linux.
          linuxRuntimeLibs = with pkgs; [
            vulkan-loader wayland libxkbcommon fontconfig freetype
            xorg.libX11 xorg.libxcb xorg.libXcursor xorg.libXrandr xorg.libXi
            alsa-lib pipewire
          ];

          # ort 2.0.0-rc.15 targets ONNX Runtime 1.31; nixpkgs is older and
          # its macOS build lacks CoreML, so use the official release where
          # one exists (docs/decisions/0009-onnxruntime.md).
          ortVersion = "1.31.0";
          ortReleases = {
            aarch64-darwin = {
              name = "osx-arm64";
              sha256 = "1031b36dd3bdaa8d976a6130d9cf2a05e9be320b93ae804f8427b91dc3432afc";
            };
            x86_64-linux = {
              name = "linux-x64";
              sha256 = "cc5c72baf5ae5c8238a6841f0897227be2d02826b9cf98eaf02fdefaeeb03a57";
            };
            aarch64-linux = {
              name = "linux-aarch64";
              sha256 = "c5b8b3cca31f3d643a3b313b8f42e2d0f16a4fcefd02cda343d97bd5306afaab";
            };
          };
          onnxruntime =
            if ortReleases ? ${system} then
              let
                r = ortReleases.${system};
              in
              stdenv.mkDerivation {
                pname = "onnxruntime-bin";
                version = ortVersion;
                src = pkgs.fetchurl {
                  url = "https://github.com/microsoft/onnxruntime/releases/download/v${ortVersion}/onnxruntime-${r.name}-${ortVersion}.tgz";
                  inherit (r) sha256;
                };
                nativeBuildInputs = lib.optionals stdenv.hostPlatform.isLinux [ pkgs.autoPatchelfHook ];
                buildInputs = lib.optionals stdenv.hostPlatform.isLinux [ stdenv.cc.cc.lib ];
                dontConfigure = true;
                dontBuild = true;
                installPhase = ''
                  mkdir -p $out
                  cp -r lib include $out/
                  rm -rf $out/lib/*.dSYM
                '';
              }
            else
              # No official build for this system (x86_64-darwin): best effort.
              pkgs.onnxruntime;

          ortLib = "${onnxruntime}/lib/libonnxruntime${stdenv.hostPlatform.extensions.sharedLibrary}";
        in
        {
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              rust
              pkg-config
              cmake
              # pyzmq and msgpack run the svs.io reference backend
              # (tests/svsio, crates/cantor-backends/tests/svsio.rs).
              (python3.withPackages (ps: [ ps.pyzmq ps.msgpack ]))
              maturin
              wasm-tools
              clap-validator
              nixfmt
              # Test fixtures are downloaded by checksum and unzipped.
              curl
              unzip
              # npx runs the MCP conformance suite (`cargo xtask conformance`).
              nodejs_22
            ];

            buildInputs =
              [ onnxruntime ]
              ++ lib.optionals stdenv.hostPlatform.isLinux linuxRuntimeLibs
              # Use the newest apple-sdk_* your nixpkgs provides.
              ++ lib.optionals stdenv.hostPlatform.isDarwin [ pkgs.apple-sdk_15 ];

            env = {
              # ort uses the `load-dynamic` feature and loads this library.
              ORT_DYLIB_PATH = ortLib;
              # bindgen (pipewire-sys and others) needs libclang.
              LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            };

            shellHook = ''
              ${lib.optionalString stdenv.hostPlatform.isLinux ''
                export LD_LIBRARY_PATH=${lib.makeLibraryPath linuxRuntimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
              ''}
              echo "cantor dev shell: $(rustc --version)"
            '';
          };

          # Latest stable Rust, for crates that must build without nightly
          # (cantor-plugin-sdk). CI: `nix develop .#stable --command ...`.
          devShells.stable = pkgs.mkShell {
            nativeBuildInputs = [
              (fenix.packages.${system}.combine [
                stableRust
                # Phonemizer plugins are WASI p2 components.
                fenix.packages.${system}.targets.wasm32-wasip2.stable.rust-std
              ])
            ];
            shellHook = ''
              echo "cantor stable shell: $(rustc --version)"
            '';
          };

          formatter = pkgs.nixfmt;
        };
    };
}
