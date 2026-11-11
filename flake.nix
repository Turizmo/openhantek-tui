{
  description = "openhantek-tui: terminal oscilloscope for the Hantek DSO-6022BE";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  inputs.crane.url = "github:ipetkov/crane";

  outputs = { self, nixpkgs, crane, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      craneLib = crane.mkLib pkgs;
      # The scope needs firmware after each power-up. We use the open
      # firmware from OpenHantek6022 (GPL-3.0). It is loaded from the Nix
      # store at run time, so it is not copied into this repo.
      firmware = "${pkgs.openhantek6022.src}/openhantek/res/firmware/dso6022be-firmware.hex";

      # Only the Rust sources, Cargo.toml and Cargo.lock. Edits to the
      # README or the flake do not rebuild the app.
      src = craneLib.cleanCargoSource ./.;

      commonArgs = {
        inherit src;
        strictDeps = true;
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs = [ pkgs.libusb1 ];
        OPENHANTEK_TUI_FIRMWARE = firmware;
      };

      # The dependency crates, built once in their own derivation. A
      # source edit only rebuilds the app crate on top of these.
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      openhantek-tui = craneLib.buildPackage (commonArgs // {
        inherit cargoArtifacts;
        # Runs `cargo test`; the hardware tests are #[ignore]
        doCheck = true;
        meta = {
          mainProgram = "openhantek-tui";
          description = "Terminal oscilloscope for the Hantek DSO-6022BE";
          license = pkgs.lib.licenses.gpl3Plus;
        };
      });
    in
    {
      packages.${system}.default = openhantek-tui;

      apps.${system}.default = {
        type = "app";
        program = "${openhantek-tui}/bin/openhantek-tui";
      };

      # `nix flake check`: the build with its tests, plus clippy
      checks.${system} = {
        inherit openhantek-tui;
        clippy = craneLib.cargoClippy (commonArgs // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--all-targets";
        });
      };

      # `nix develop` gives Rust, plus libusb for talking to the scope
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          rustc
          rustfmt
          clippy
          rust-analyzer
          pkg-config
          libusb1
        ];
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        OPENHANTEK_TUI_FIRMWARE = firmware;
      };
    };
}
