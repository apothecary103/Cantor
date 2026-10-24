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

          # Libraries GPUI and cpal load at runtime on Linux.
          linuxRuntimeLibs = with pkgs; [
            vulkan-loader wayland libxkbcommon fontconfig freetype
            xorg.libX11 xorg.libxcb xorg.libXcursor xorg.libXrandr xorg.libXi
            alsa-lib pipewire
          ];

          ortLib = "${pkgs.onnxruntime}/lib/libonnxruntime${stdenv.hostPlatform.extensions.sharedLibrary}";
        in
        {
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              rust
              pkg-config
              cmake
              python3
              maturin
              wasm-tools
              nixfmt
            ];

            buildInputs =
              [ pkgs.onnxruntime ]
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
              (fenix.packages.${system}.stable.withComponents [
                "cargo"
                "rustc"
                "rust-std"
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
