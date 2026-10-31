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

          # ------------------------------------------------------------------
          # Android (phone) target — `nix develop .#android`.
          #
          # nixpkgs' androidenv supplies the SDK the generated Gradle project
          # asks for: platform android-37.0, build-tools 37.0.0, platform-tools,
          # cmdline-tools and NDK r29 (27.0.12077973-style pins are not in this
          # nixpkgs; 29.0.14206865 is the GA release of the r29 line the Tauri
          # 2.12 template is written against). `licenseAccepted = true` is the
          # Google SDK license acceptance: androidenv then unpacks the SDK and
          # writes `licenses/` into it, which AGP checks instead of prompting.
          #
          # The Rust side cannot come from nixpkgs: its `rustc` is a source
          # build with no Android std and stable cannot `-Z build-std`, so the
          # official 1.98.1 distribution tarballs are merged into one prefix
          # whose `lib/rustlib` lists the four Android targets — the same check
          # `tauri android` performs before it builds. Pinning the version keeps
          # the phone build on the compiler the desktop shell ships.
          # ------------------------------------------------------------------
          # AGP 9.3.1's default build-tools is 36.0.0 (the project does not pin
          # one), so that is what the SDK must carry. Two platforms are needed:
          # `android-37.0` is what the app's `compileSdk = 37` resolves to, and
          # `android-36` is what the Tauri Android AARs (tauri, the plugins)
          # compile their own Java against.
          androidNdkVersion = "29.0.14206865";
          androidBuildToolsVersion = "36.0.0";
          androidPlatformVersions = [
            "37.0"
            "36"
          ];

          # androidenv's SDK and NDK are unfree, and unpacking them needs the
          # Google license accepted up front: this instance is scoped to the
          # phone shell, so the desktop shells keep nixpkgs' defaults.
          androidPkgs = import inputs.nixpkgs {
            inherit (pkgs.stdenv.hostPlatform) system;
            config = {
              allowUnfree = true;
              android_sdk.accept_license = true;
            };
          };

          androidSdk =
            (androidPkgs.androidenv.composeAndroidPackages {
              includeNDK = true;
              ndkVersions = [ androidNdkVersion ];
              platformVersions = androidPlatformVersions;
              buildToolsVersions = [ androidBuildToolsVersion ];
              includeCmake = false;
              includeEmulator = false;
              includeSystemImages = false;
              includeSources = false;
            }).androidsdk;

          # An official rust-lang.org toolchain, merged: rustc for the build
          # host plus std for the host and for the four Android targets. The
          # tarballs are the distribution's own; the only work done here is
          # unpacking them into one sysroot and pointing their dynamic loader
          # at this store prefix (autoPatchelfHook), since NixOS has no
          # /lib64/ld-linux-x86-64.so.2.
          rustAndroidToolchain = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "rust-android-toolchain";
            version = "1.98.1";

            srcs = [
              (pkgs.fetchurl {
                name = "rustc.tar.xz";
                url = "https://static.rust-lang.org/dist/rustc-1.98.1-x86_64-unknown-linux-gnu.tar.xz";
                hash = "sha256-6XTwNrKFZfN8DzvZLd76gJvuFsBPnc8Hue2W4Fqq98Q=";
              })
              (pkgs.fetchurl {
                name = "rust-std-host.tar.xz";
                url = "https://static.rust-lang.org/dist/rust-std-1.98.1-x86_64-unknown-linux-gnu.tar.xz";
                hash = "sha256-+j/0UBcqFsAmlEAwIwxQaZR6+TxyjZF5lx1E5eDPtWE=";
              })
              (pkgs.fetchurl {
                name = "rust-std-aarch64-linux-android.tar.xz";
                url = "https://static.rust-lang.org/dist/rust-std-1.98.1-aarch64-linux-android.tar.xz";
                hash = "sha256-18SUm7d7AHvtGIybFbgmejIi3JucUX8F0Ymfnohft54=";
              })
              (pkgs.fetchurl {
                name = "rust-std-armv7-linux-androideabi.tar.xz";
                url = "https://static.rust-lang.org/dist/rust-std-1.98.1-armv7-linux-androideabi.tar.xz";
                hash = "sha256-M06vox8/TldGMG4gdhVmMF5eLDoMrwTlTbTnM9MDvmI=";
              })
              (pkgs.fetchurl {
                name = "rust-std-i686-linux-android.tar.xz";
                url = "https://static.rust-lang.org/dist/rust-std-1.98.1-i686-linux-android.tar.xz";
                hash = "sha256-26PVccO1jGQ31TPkQgKuUEZXICtohysVnkYt4OyWqrc=";
              })
              (pkgs.fetchurl {
                name = "rust-std-x86_64-linux-android.tar.xz";
                url = "https://static.rust-lang.org/dist/rust-std-1.98.1-x86_64-linux-android.tar.xz";
                hash = "sha256-/lH3/XNVOxl7XEvndt9oGViiP/ZbyMld0uF20URDpQY=";
              })
            ];

            nativeBuildInputs = [ pkgs.autoPatchelfHook ];

            # rustc's own DT_NEEDED set is glibc, libgcc_s and the libLLVM it
            # ships (a $ORIGIN rpath); zlib/libstdc++ cover the tools in the
            # tarball that link them.
            buildInputs = [
              pkgs.stdenv.cc.cc.lib
              pkgs.zlib
            ];

            dontUnpack = true;
            dontConfigure = true;
            dontBuild = true;
            dontStrip = true;

            # Every tarball unpacks to `<component>-<version>-<target>/rust-<component>/`;
            # merging those payloads puts rustc and each target's std in the one
            # `lib/rustlib` a sysroot is expected to have.
            installPhase = ''
              runHook preInstall
              mkdir -p "$out"
              for tarball in $srcs; do
                unpacked=$(mktemp -d)
                tar -xJf "$tarball" -C "$unpacked"
                for payload in "$unpacked"/*/rust*/; do
                  cp -a "$payload"/. "$out"/
                done
                rm -rf "$unpacked"
              done
              runHook postInstall
            '';

            meta = {
              description = "Official Rust ${finalAttrs.version} with the Android standard libraries";
              homepage = "https://static.rust-lang.org/";
              license = with pkgs.lib.licenses; [
                mit
                asl20
              ];
              platforms = [ "x86_64-linux" ];
            };
          });
        in
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
              # `mupdf-sys` (PDF rasterisation for attachments) runs bindgen in
              # its build script, and bindgen shells out to `clang
              # --print-resource-dir` and dlopens libclang. Both come from this
              # package; LIBCLANG_PATH below points bindgen at its lib dir.
              clang
            ] ++ pkgs.lib.optionals isLinux [
              # Qt toolchain for the Qt runtime: the merged tree plus the shim.
              qmakeMerged
              qtMerged
              cmake
              mold # host link step, via the clang driver in the shellHook
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

              # bindgen (mupdf-sys) dlopens libclang from here: `clang` on PATH
              # is a wrapper with no lib dir, so point at nixpkgs' libclang
              # package, which ships libclang.so.
              export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"

              # Nix's rustc ships no rust-lld, so the wasm32 target links through
              # lld's wasm-ld, which this shell provides (the package and
              # `devShells.plugin` set the same thing).
              export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld
            '' + pkgs.lib.optionalString isLinux ''
              # qt-build-utils prefers $QMAKE over PATH and searches for qmake6
              # first, so pin the shim explicitly rather than rely on PATH order.
              export QMAKE="${qmakeMerged}/bin/qmake"

              # mold instead of the GNU linker for the host binary: the app
              # links one large executable, and mold finishes it in a fraction
              # of the time. `clang` is the driver, `mold` above supplies
              # ld.mold. Per-target variables rather than RUSTFLAGS so a
              # caller's own RUSTFLAGS still apply. (lld keeps its wasm32 job;
              # macOS links with its own ld64, so this stays Linux-only.)
              export CARGO_TARGET_${pkgs.lib.toUpper (builtins.replaceStrings [ "-" "." ] [ "_" "_" ] pkgs.stdenv.hostPlatform.config)}_LINKER=clang
              export CARGO_TARGET_${pkgs.lib.toUpper (builtins.replaceStrings [ "-" "." ] [ "_" "_" ] pkgs.stdenv.hostPlatform.config)}_RUSTFLAGS="-Clink-arg=-fuse-ld=mold"
            '';
          };

          # `nix develop .#android`: everything `npm run tauri android build`
          # needs and nothing from the desktop Qt/GTK stack. The SDK, NDK, JDK
          # and Rust toolchain are the ones defined above; the variables below
          # are the names Tauri's Android commands and AGP read.
          devShells.android = pkgs.mkShell {
            name = "ledger-app-android";

            packages = [
              rustAndroidToolchain # official rustc + the four Android stds
              pkgs.cargo # nixpkgs' cargo, driving that rustc
              androidSdk # SDK pieces, licenses accepted
              pkgs.jdk17 # Gradle 9.6.1 + AGP 9.3.1 both run on 17
              pkgs.cmake
              pkgs.nodejs # `npm run tauri`, and the Gradle build calls it back
              pkgs.pkg-config
              pkgs.stdenv.cc # host cc for build scripts and proc macros
              pkgs.binutils
              pkgs.cargo-tauri
              pkgs.unzip # inspecting the APK afterwards
            ];

            ANDROID_HOME = "${androidSdk}/libexec/android-sdk";
            ANDROID_SDK_ROOT = "${androidSdk}/libexec/android-sdk";
            NDK_HOME = "${androidSdk}/libexec/android-sdk/ndk/${androidNdkVersion}";
            ANDROID_NDK_HOME = "${androidSdk}/libexec/android-sdk/ndk/${androidNdkVersion}";
            ANDROID_NDK_ROOT = "${androidSdk}/libexec/android-sdk/ndk/${androidNdkVersion}";
            JAVA_HOME = "${pkgs.jdk17}";
            CMAKE = "${pkgs.cmake}/bin/cmake";

            shellHook = ''
              # Gradle's own caches (the 9.6.1 distribution, then AGP's Maven
              # dependencies) stay on the repo volume rather than in ~/.gradle.
              export GRADLE_USER_HOME="''${GRADLE_USER_HOME:-$PWD/.gradle-android}"

              # The NDK's clang wrappers are the linkers named in
              # src-tauri/.cargo/config.toml, and build-tools/platform-tools
              # put aapt2, apksigner and adb on PATH for inspection. The host
              # `cc` in `packages` is for build scripts and proc macros only:
              # the CC_*/CXX_* variables below keep every target's C compiler
              # on the NDK.
              export PATH="$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$ANDROID_HOME/build-tools/${androidBuildToolsVersion}:$ANDROID_HOME/platform-tools:$PATH"

              # cc-rs/ring/libgit2 build their C for the phone: name the NDK
              # wrappers for each target rather than rely on its heuristics.
              export CC_aarch64_linux_android=aarch64-linux-android24-clang
              export CXX_aarch64_linux_android=aarch64-linux-android24-clang++
              export AR_aarch64_linux_android=llvm-ar
              export CC_armv7_linux_androideabi=armv7a-linux-androideabi24-clang
              export CXX_armv7_linux_androideabi=armv7a-linux-androideabi24-clang++
              export AR_armv7_linux_androideabi=llvm-ar
              export CC_i686_linux_android=i686-linux-android24-clang
              export CXX_i686_linux_android=i686-linux-android24-clang++
              export AR_i686_linux_android=llvm-ar
              export CC_x86_64_linux_android=x86_64-linux-android24-clang
              export CXX_x86_64_linux_android=x86_64-linux-android24-clang++
              export AR_x86_64_linux_android=llvm-ar
            '';
          };
        };
    };
}
