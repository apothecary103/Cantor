{
  description = "Cantor – UTAU-style singing synthesis workstation (dev environment)";

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
        in
        {
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              rust
              pkg-config
              nixfmt
            ];

            # Use the newest apple-sdk_* your nixpkgs provides.
            buildInputs = lib.optionals stdenv.hostPlatform.isDarwin [ pkgs.apple-sdk_15 ];

            shellHook = ''
              echo "cantor dev shell: $(rustc --version)"
            '';
          };

          formatter = pkgs.nixfmt;
        };
    };
}
