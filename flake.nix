# SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
#
# SPDX-License-Identifier: AGPL-3.0-or-later

{
  description = "LedgerWebApp devshell, and the envelope plugin as a package";

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
        { pkgs, self', ... }:
        {
          # `nix build .#rustledger-envelope` (or plain `nix build`) produces the
          # plugin as a WASM module, built offline from the crate's Cargo.lock and
          # verified by its own test suite: the same artifact CI attaches to a
          # release. nixpkgs' rustc already ships the wasm32 standard library, so
          # no rust overlay is needed — only lld, for the link step.
          packages.rustledger-envelope = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "rustledger-envelope";
            version = "0.1.0";

            src = ./plugins/rustledger-envelope;

            cargoDeps = pkgs.rustPlatform.importCargoLock {
              lockFile = ./plugins/rustledger-envelope/Cargo.lock;
            };

            nativeBuildInputs = with pkgs; [
              cargo
              rustc
              lld
            ];

            # Nix's rustc has the wasm32 standard library but no rust-lld, so the
            # link step goes through lld's wasm-ld.
            CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER = "wasm-ld";

            # Cargo must not reach the network inside the sandbox: point it at the
            # vendored sources the lock file pins.
            configurePhase = ''
              runHook preConfigure

              export CARGO_HOME="$TMPDIR/cargo-home"
              mkdir -p "$CARGO_HOME"
              cat > "$CARGO_HOME/config.toml" <<EOF
              [source.crates-io]
              replace-with = "vendored-sources"

              [source.vendored-sources]
              directory = "${finalAttrs.cargoDeps}"
              EOF

              runHook postConfigure
            '';

            buildPhase = ''
              runHook preBuild
              cargo build --offline --locked --release --target wasm32-unknown-unknown
              runHook postBuild
            '';

            # The test suite runs natively — a wasm test binary cannot execute
            # here — so the target is left to the host for this phase.
            doCheck = true;
            checkPhase = ''
              runHook preCheck
              cargo test --offline --locked --all-targets
              runHook postCheck
            '';

            installPhase = ''
              runHook preInstall
              install -Dm644 \
                target/wasm32-unknown-unknown/release/rustledger_envelope.wasm \
                "$out/lib/rustledger_envelope.wasm"
              runHook postInstall
            '';

            meta = {
              description = "Envelope budgeting for rustledger, as a WASM plugin";
              license = pkgs.lib.licenses.agpl3Plus;
              platforms = pkgs.lib.platforms.unix;
            };
          });

          packages.default = self'.packages.rustledger-envelope;

          # `nix develop .#plugin` for working on the crate alone: no webkit, no
          # node, no tauri — just the compiler, the wasm linker and rustfmt.
          devShells.plugin = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              cargo
              rustc
              rustfmt
              lld
            ];

            shellHook = ''
              export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld
            '';
          };

          devShells.default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              pkg-config
              wrapGAppsHook4
              cargo
              cargo-tauri # Optional, Only needed if Tauri doesn't work through the traditional way.
              nodejs # Optional, this is for if you have a js frontend
              rustc # Needed for dev server (npm tauri dev)
              lld # wasm32-unknown-unknown linker for plugins/rustledger-envelope
              gst_all_1.gst-plugins-base # Optional, if you get `GStreamer element appsink not found. Please install it.`
            ];

            buildInputs = with pkgs; [
              librsvg
              webkitgtk_4_1
            ];

            shellHook = ''
              export XDG_DATA_DIRS="$GSETTINGS_SCHEMAS_PATH" # Needed on Wayland to report the correct display scale

              # Nix's rustc ships no rust-lld, so the wasm32 target links through
              # lld's wasm-ld, which this shell provides (the package and
              # `devShells.plugin` set the same thing).
              export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld
            '';
          };
        };
    };
}
