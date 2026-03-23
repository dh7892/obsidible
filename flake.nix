{
  description = "Obsidible - Bidirectional sync between Obsidian vaults and reMarkable tablets";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Use latest stable Rust (needs edition 2024 support = 1.85+)
        rustToolchain = pkgs.rust-bin.stable.latest.default;

        # Package rmc (not in nixpkgs) as a Python application
        rmc = pkgs.python3Packages.buildPythonApplication rec {
          pname = "rmc";
          version = "0.3.0";
          format = "pyproject";

          src = pkgs.fetchPypi {
            inherit pname version;
            hash = "sha256-V6/hTVZpQIW2o4KqK5O3uG6yHpPnILFqgpkKoNZRPcs=";
          };

          build-system = with pkgs.python3Packages; [
            setuptools
          ];

          dependencies = with pkgs.python3Packages; [
            click
            rmscene
          ];

          meta = with pkgs.lib; {
            description = "Convert to/from v6 .rm files from the reMarkable tablet";
            homepage = "https://github.com/ricklupton/rmc";
            license = licenses.mit;
          };
        };

        # Runtime dependencies that obsidible shells out to
        runtimeDeps = [
          pkgs.rmapi         # reMarkable cloud CLI
          rmc                 # .rm file conversion (Python)
          pkgs.librsvg        # SVG to PNG (rsvg-convert)
          pkgs.typst          # Markdown to PDF
          pkgs.poppler_utils  # PDF to images (pdftoppm, pdfinfo)
          pkgs.imagemagick    # Image compositing (magick)
        ];

        # Native build inputs for compiling the Rust project
        nativeBuildInputs = with pkgs; [
          pkg-config
        ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
          pkgs.darwin.apple_sdk.frameworks.Security
          pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
        ];

        buildInputs = with pkgs; [
          openssl
        ];

      in
      {
        # Development shell with all tools available
        devShells.default = pkgs.mkShell {
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ [
            rustToolchain
            pkgs.cargo-watch
            pkgs.rust-analyzer
          ] ++ runtimeDeps;

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          shellHook = ''
            echo "obsidible dev shell"
            echo "  rust: $(rustc --version)"
            echo "  rmapi: $(rmapi version 2>/dev/null || echo 'available')"
            echo "  rmc: $(rmc --version 2>/dev/null || echo 'available')"
            echo "  typst: $(typst --version)"
            echo "  rsvg-convert: $(rsvg-convert --version)"
          '';
        };

        # The obsidible package itself
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "obsidible";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          inherit buildInputs;

          # Wrap the binary so runtime deps are on PATH
          nativeBuildInputs = nativeBuildInputs ++ [ pkgs.makeWrapper ];

          postInstall = ''
            wrapProgram $out/bin/obsidible \
              --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps}
          '';

          meta = with pkgs.lib; {
            description = "Bidirectional sync between Obsidian vaults and reMarkable tablets";
            license = licenses.mit;
            mainProgram = "obsidible";
          };
        };
      }
    );
}
