# SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
#
# SPDX-License-Identifier: AGPL-3.0-or-later

{
  description = "LedgerWebApp devshell";

  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      perSystem =
        { pkgs, ... }:
        let
          isLinux = pkgs.stdenv.hostPlatform.isLinux;

          # qtwebengine installs into its own prefix; `qt-build-utils` (used by
          # tauri-runtime-qt) resolves modules through `qmake -query`, which only
          # knows qtbase's. Merge the module prefixes and shim qmake so module
          # lookups see the merged tree (TauriQt docs/nix-qt-gotchas.md).
          qtMerged = pkgs.symlinkJoin {
            name = "qt-merged";
            paths = with pkgs.kdePackages; [
              qtbase
              qtwebengine
              qtdeclarative
              qtwebchannel
              qtpositioning
            ];
          };

          qmakeMerged = pkgs.writeShellScriptBin "qmake" ''
            real="${pkgs.kdePackages.qtbase}/bin/qmake"
            base="${pkgs.kdePackages.qtbase}"
            merged="${qtMerged}"
            if [ "''${1:-}" = "-query" ]; then
              v="$("$real" -query "''${2:-}")"
              echo "''${v//$base/$merged}"
            else
              exec "$real" "$@"
            fi
          '';
        in
        {
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              pkg-config
              wrapGAppsHook4
              cargo
              cargo-tauri # Optional, Only needed if Tauri doesn't work through the traditional way.
              nodejs # Optional, this is for if you have a js frontend
              rustc # Needed for dev server (npm tauri dev)
              gst_all_1.gst-plugins-base # Optional, if you get `GStreamer element appsink not found. Please install it.`
            ] ++ pkgs.lib.optionals isLinux [
              # Qt toolchain for the Qt runtime: the merged tree plus the shim.
              qmakeMerged
              qtMerged
              cmake
            ];

            buildInputs = with pkgs; [
              librsvg
            ] ++ pkgs.lib.optionals isLinux [
              # tauri-runtime links the GTK stack unconditionally on Linux, so
              # these .pc files must be on PKG_CONFIG_PATH for the -sys crates
              # (`atk` is not implied by gtk3). QtWebEngine comes with the Qt
              # runtime.
              glib
              gtk3
              webkitgtk_4_1
              atk
              kdePackages.qtbase
              kdePackages.qtwebengine
              libGL
            ];

            shellHook = ''
              export XDG_DATA_DIRS="$GSETTINGS_SCHEMAS_PATH" # Needed on Wayland to report the correct display scale
            '' + pkgs.lib.optionalString isLinux ''
              # qt-build-utils prefers $QMAKE over PATH and searches for qmake6
              # first, so pin the shim explicitly rather than rely on PATH order.
              export QMAKE="${qmakeMerged}/bin/qmake"
            '';
          };
        };
    };
}
