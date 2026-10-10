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
            ];

            buildInputs = with pkgs; [
              librsvg
              webkitgtk_4_1
            ];

            shellHook = ''
              export XDG_DATA_DIRS="$GSETTINGS_SCHEMAS_PATH" # Needed on Wayland to report the correct display scale
            '';
          };
        };
    };
}
