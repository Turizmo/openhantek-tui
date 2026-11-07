{
  description = "openhantek-tui: terminal oscilloscope for the Hantek DSO-6022BE";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      # The scope needs firmware after each power-up. We use the open
      # firmware from OpenHantek6022 (GPL-3.0). It is loaded from the Nix
      # store at run time, so it is not copied into this repo.
      firmware = "${pkgs.openhantek6022.src}/openhantek/res/firmware/dso6022be-firmware.hex";
    in
    {
      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "openhantek-tui";
        version = "0.1.0";
        src = self;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs = [ pkgs.libusb1 ];
        OPENHANTEK_TUI_FIRMWARE = firmware;
        meta.mainProgram = "openhantek-tui";
      };

      apps.${system}.default = {
        type = "app";
        program = "${self.packages.${system}.default}/bin/openhantek-tui";
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
