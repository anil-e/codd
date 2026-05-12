{
  description = "Gitte Dev Shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
  inputs.systems.url = "github:nix-systems/default";
  inputs.flake-utils = {
    url = "github:numtide/flake-utils";
    inputs.systems.follows = "systems";
  };

  inputs.rust-overlay.url = "github:oxalica/rust-overlay";

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustStable = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rustfmt"
            "clippy"
          ];
        };

        rustNightly = pkgs.rust-bin.selectLatestNightlyWith (
          t: t.default.override { extensions = [ "rustfmt" ]; }
        );

        rustfmtWrapper = pkgs.writeShellScriptBin "rustfmt" ''
          exec "${rustNightly}/bin/rustfmt" "$@"
        '';

        cargo = pkgs.writeShellScriptBin "cargo" ''
          set -euo pipefail

          run_tc() {
            local tc_bin="$1"
            shift
            exec env \
              PATH="$tc_bin:$PATH" \
              RUSTFMT="$tc_bin/rustfmt" \
              "$@"
          }

          if [ "''${1:-}" = "+nightly" ]; then
            shift
            run_tc "${rustNightly}/bin" "${rustNightly}/bin/cargo" "$@"
          elif [ "''${1:-}" = "+stable" ]; then
            shift
            run_tc "${rustStable}/bin" "${rustStable}/bin/cargo" "$@"
          elif [ "''${1:-}" = "fmt" ]; then
            run_tc "${rustNightly}/bin" "${rustNightly}/bin/cargo" "$@"
          else
            run_tc "${rustStable}/bin" "${rustStable}/bin/cargo" "$@"
          fi
        '';
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            gtk4
            libadwaita
            glib
            gettext
            openssl
            gsettings-desktop-schemas
            gtksourceview5
          ];

          nativeBuildInputs = with pkgs; [
            pkg-config
            meson
            ninja
            desktop-file-utils
            appstream
            wrapGAppsHook4
          ];

          shellHook = ''
            export XDG_DATA_DIRS="${pkgs.gtk4}/share:${pkgs.libadwaita}/share:${pkgs.gsettings-desktop-schemas}/share:${pkgs.glib}/share:$XDG_DATA_DIRS"
          '';

          packages = with pkgs; [
            bashInteractive
            cargo
            cargo-outdated
            rustfmtWrapper
            rustStable
            rustNightly
            rust-analyzer
            clang
            gcc
            flatpak-builder
            (python3.withPackages (ps: with ps; [
              aiohttp
              tomlkit
            ]))
          ];
        };
      }
    );
}
