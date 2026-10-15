# SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
#
# SPDX-License-Identifier: AGPL-3.0-or-later

{
  description = "LedgerWebApp devshell, and the envelope plugin as a package";

  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Android rust std targets: nixpkgs' rustc ships std only for the host,
    # wasm and bpf.
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } (
      { inputs, ... }:
      {
        systems = [
          "x86_64-linux"
          "aarch64-linux"
          "aarch64-darwin"
          "x86_64-darwin"
        ];
        perSystem =
          {
            lib,
            pkgs,
            self',
            system,
            ...
          }:
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

            # Android builds need the SDK/NDK, which Google ships as
            # x86_64-linux binaries only — so the android devshell and the APK
            # derivation are x86_64-linux artifacts: build them on an x86_64
            # host (CI), or under binfmt x86_64 emulation on an aarch64 host.
            # SDK versions track what tauri-cli's template writes into
            # src-tauri/gen/android (compileSdk 37, AGP 9.3).
            pkgsAndroid = import inputs.nixpkgs {
              inherit system;
              config.android_sdk.accept_license = true;
              # The SDK packages are license-gated in nixpkgs (unfree) — the
              # Google SDK license acceptance above is what makes this ok.
              config.allowUnfree = true;
            };
            androidComposition = pkgsAndroid.androidenv.composeAndroidPackages {
              platformVersions = [ "37" ];
              buildToolsVersions = [
                "37.0.0"
                "36.0.0"
              ];
              includeNDK = true;
              ndkVersions = [ "29.0.14206865" ];
              abiVersions = [ "arm64-v8a" ];
              includeEmulator = false;
              includeSystemImages = false;
            };
            androidHome = "${androidComposition.androidsdk}/libexec/android-sdk";
            ndkVersion = "29.0.14206865";
            ndkHome = "${androidHome}/ndk/${ndkVersion}";
            androidAbis = [ "arm64-v8a" ];
            androidArchs = [ "arm64" ];
            androidTargets = [ "aarch64-linux-android" ];

            rustAndroidToolchain = inputs.fenix.packages.${system}.combine (
              [
                inputs.fenix.packages.${system}.stable.cargo
                inputs.fenix.packages.${system}.stable.rustc
                inputs.fenix.packages.${system}.stable.rust-std
              ]
              ++ map (target: inputs.fenix.packages.${system}.targets.${target}.stable.rust-std) androidTargets
            );

            # tauri-cli probes `rustup target` before building; the toolchain
            # already ships the android targets, so answer its probe and stop.
            rustupShim = pkgsAndroid.writeShellScriptBin "rustup" ''
              if [[ "$1 $2" == "target list" ]]; then
                printf '%s\n' ${lib.concatStringsSep " " androidTargets}
              fi
            '';

            # Gradle fetches the wrapper distribution, AGP, the Kotlin plugin
            # and the androidx artifacts from the network. Gradle's own cache
            # cannot be hashed: its index stores (module metadata, file hashes,
            # execution history) embed host paths and timestamps, so a hash over
            # the whole GRADLE_USER_HOME changes between identical runs. This
            # derivation instead emits only reproducible bytes — the downloaded
            # artifacts in plain maven layout, plus the extracted gradle
            # distribution — and the APK build resolves against the layout
            # through the init script below, so no cache index is involved and
            # the hash is stable by construction.
            gradleDeps =
              pkgsAndroid.runCommand "ledgerapp-gradle-deps"
                {
                  outputHashAlgo = "sha256";
                  outputHashMode = "recursive";
                  outputHash = "sha256-oGXS2ztfvQQK4tkC9q2eogkpdchHhV3gwedLk4OUZOQ=";
                  nativeBuildInputs = [ pkgsAndroid.jdk21_headless ];
                }
                ''
                  set -euo pipefail
                  export HOME=$TMPDIR/home
                  mkdir -p "$HOME"
                  cp -r ${./src-tauri/gen/android} gradle-project
                  chmod -R u+w gradle-project
                  cd gradle-project
                  # tauri-build generates these during the cargo build (they wire
                  # the tauri .aar subprojects); this derivation only downloads
                  # dependencies, so empty stubs are enough to configure here.
                  touch tauri.settings.gradle app/tauri.build.gradle.kts

                  # A throwaway cache: it is where gradle downloads into, and it
                  # is deliberately not part of the output.
                  export GRADLE_USER_HOME=$TMPDIR/gradle-home
                  ./gradlew --no-daemon \
                    -PabiList=${lib.concatStringsSep "," androidAbis} \
                    -ParchList=${lib.concatStringsSep "," androidArchs} \
                    -PtargetList=${lib.concatStringsSep "," androidTargets} \
                    :app:dependencies

                  # The gradle distribution the wrapper script needs. Extracted
                  # from the distribution zip, so it is byte-reproducible.
                  mkdir -p $out
                  cp -r "$GRADLE_USER_HOME/wrapper" $out/wrapper

                  # Re-lay the downloaded artifacts as a maven repository:
                  #   cache: <group>/<artifact>/<version>/<sha1>/<file>
                  #   repo:  <group as path>/<artifact>/<version>/<file>
                  # Only the artifacts survive — jars, aars, poms, gradle
                  # module metadata — all of them immutable downloads.
                  base="$GRADLE_USER_HOME/caches/modules-2/files-2.1"
                  find "$base" -type f > "$TMPDIR/artifacts.txt"
                  while IFS= read -r f; do
                    rel=''${f#"$base"/}
                    file=''${rel##*/}
                    rest=''${rel%/*}                     # group/artifact/version/sha1
                    gav=''${rest%/*}                     # group/artifact/version
                    version=''${gav##*/}
                    ga=''${gav%/*}                       # group/artifact
                    artifact=''${ga##*/}
                    group=''${ga%/*}                     # com.example
                    groupPath=''$(printf '%s' "$group" | tr '.' '/')
                    mkdir -p "$out/maven/$groupPath/$artifact/$version"
                    cp -p "$f" "$out/maven/$groupPath/$artifact/$version/$file"
                  done < "$TMPDIR/artifacts.txt"
                '';

            # The APK itself: `tauri android build` drives gradle, which drives
            # back into tauri-cli (`android-studio-script`) for the cargo build,
            # so one invocation covers rust, frontend and packaging. Debug APK:
            # installable without signing keys; release signing is a follow-up
            # that needs a keystore.
            ledgerapp-android = pkgsAndroid.stdenv.mkDerivation (finalAttrs: {
              pname = "ledger-app-android";
              version = (builtins.fromJSON (builtins.readFile ./package.json)).version;

              src = ./.;

              npmDeps = pkgsAndroid.fetchNpmDeps {
                inherit (finalAttrs) src;
                name = "ledgerapp-npm-deps";
                hash = "sha256-HnC7UKrLquphBEZCDnuk7oMYiQpQ3vxmyS7PgotMcEI=";
              };
              cargoDeps = pkgsAndroid.rustPlatform.importCargoLock {
                lockFile = ./src-tauri/Cargo.lock;
                # Git dependency (the Qt runtime crate, linux-only but still
                # part of the shared lockfile).
                outputHashes."tauri-runtime-qt-0.1.0" = "sha256-N/0DRCo2gIJ09TH/2/n64kdDUv6bzG0JY6vovnEhWL0=";
              };

              nativeBuildInputs = with pkgsAndroid; [
                nodejs
                jdk21_headless
                rustAndroidToolchain
                rustupShim
                androidComposition.androidsdk
              ];

              env = {
                JAVA_HOME = "${pkgsAndroid.jdk21_headless.home}";
                ANDROID_HOME = androidHome;
                NDK_HOME = ndkHome;
                ANDROID_NDK_ROOT = ndkHome;
                # Point AGP at the SDK's (sandbox-patched) aapt2 instead of the
                # unpatched maven binary, and keep the daemon out of the build.
                GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${androidHome}/build-tools/37.0.0/aapt2 -Dorg.gradle.daemon=false";
                # cc-built crates (libgit2) need the NDK compiler per target.
                CC_aarch64_linux_android = "${ndkHome}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang";
                AR_aarch64_linux_android = "${ndkHome}/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar";
                CARGO_NET_OFFLINE = "true";
                CI = "true";
              };

              configurePhase = ''
                runHook preConfigure

                export HOME=$TMPDIR/home
                mkdir -p "$HOME"

                # The gradle distribution comes from the deps derivation, and an
                # init script points every repository at its maven layout, so
                # resolution is fully local and no gradle cache index is needed.
                export GRADLE_USER_HOME=$TMPDIR/gradle-home
                mkdir -p "$GRADLE_USER_HOME/init.d"
                cp -r ${gradleDeps}/wrapper "$GRADLE_USER_HOME/wrapper"
                chmod -R u+w "$GRADLE_USER_HOME/wrapper"
                cat > "$GRADLE_USER_HOME/init.d/local-repo.gradle" <<'GRADLE_INIT'
                // Replace every remote repository with the maven layout the flake
                // fetched, and keep the build offline: the sandbox has no network.
                def localRepo = uri("${gradleDeps}/maven")

                gradle.beforeSettings { settings ->
                  settings.pluginManagement.repositories.clear()
                  settings.pluginManagement.repositories.maven { url = localRepo }
                  settings.dependencyResolutionManagement.repositories.clear()
                  settings.dependencyResolutionManagement.repositories.maven { url = localRepo }
                }

                gradle.settingsEvaluated { settings ->
                  settings.gradle.startParameter.offline = true
                }

                allprojects {
                  repositories.clear()
                  repositories.maven { url = localRepo }
                  buildscript.repositories.clear()
                  buildscript.repositories.maven { url = localRepo }
                }
                GRADLE_INIT

                export CARGO_HOME=$TMPDIR/cargo-home
                mkdir -p "$CARGO_HOME"
                cat > "$CARGO_HOME/config.toml" <<EOF
                [source.crates-io]
                replace-with = "vendored-sources"

                [source.vendored-sources]
                directory = "${finalAttrs.cargoDeps}"
                EOF

                export npm_config_cache=$TMPDIR/npm-cache
                mkdir -p "$npm_config_cache"
                cp -r ${finalAttrs.npmDeps}/. "$npm_config_cache/"
                chmod -R u+w "$npm_config_cache"
                npm ci --offline --ignore-scripts

                runHook postConfigure
              '';

              buildPhase = ''
                runHook preBuild
                npm run tauri -- android build --target aarch64 --debug --apk -- --offline
                runHook postBuild
              '';

              installPhase = ''
                runHook preInstall
                mkdir -p $out
                cp src-tauri/gen/android/app/build/outputs/apk/universal/debug/*.apk $out/
                runHook postInstall
              '';

              dontFixup = true;

              meta = {
                description = "LedgerApp Android (arm64) debug APK";
                license = pkgsAndroid.lib.licenses.agpl3Plus;
                platforms = [ "x86_64-linux" ];
              };
            });
          in
          {
            packages = {
              # `nix build .#rustledger-envelope` (or plain `nix build`) produces the
              # plugin as a WASM module, built offline from the crate's Cargo.lock and
              # verified by its own test suite: the same artifact CI attaches to a
              # release. nixpkgs' rustc already ships the wasm32 standard library, so
              # no rust overlay is needed — only lld, for the link step.
              rustledger-envelope = pkgs.stdenv.mkDerivation (finalAttrs: {
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

              default = self'.packages.rustledger-envelope;
            }
            // lib.optionalAttrs (system == "x86_64-linux") {
              ledgerapp-android = ledgerapp-android;
              # The gradle cache FOD, exposed so its hash can be re-bootstrapped
              # with a plain `nix build` (a mismatch prints the expected hash).
              ledgerapp-android-gradle-deps = gradleDeps;
            };

            devShells = {
              # `nix develop .#plugin` for working on the crate alone: no webkit, no
              # node, no tauri — just the compiler, the wasm linker and rustfmt.
              plugin = pkgs.mkShell {
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

              default = pkgs.mkShell {
                nativeBuildInputs =
                  with pkgs;
                  [
                    pkg-config
                    wrapGAppsHook4
                    cargo
                    cargo-tauri # Optional, Only needed if Tauri doesn't work through the traditional way.
                    nodejs # Optional, this is for if you have a js frontend
                    rustc # Needed for dev server (npm tauri dev)
                    lld # wasm32-unknown-unknown linker for plugins/rustledger-envelope
                    gst_all_1.gst-plugins-base # Optional, if you get `GStreamer element appsink not found. Please install it.`
                  ]
                  ++ pkgs.lib.optionals isLinux [
                    # Qt toolchain for the Qt runtime: the merged tree plus the shim.
                    qmakeMerged
                    qtMerged
                    cmake
                  ];

                buildInputs =
                  with pkgs;
                  [
                    librsvg
                  ]
                  ++ pkgs.lib.optionals isLinux [
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

                  # Nix's rustc ships no rust-lld, so the wasm32 target links through
                  # lld's wasm-ld, which this shell provides (the package and
                  # `devShells.plugin` set the same thing).
                  export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld
                ''
                + pkgs.lib.optionalString isLinux ''
                  # qt-build-utils prefers $QMAKE over PATH and searches for qmake6
                  # first, so pin the shim explicitly rather than rely on PATH order.
                  export QMAKE="${qmakeMerged}/bin/qmake"
                '';
              };
            }
            // lib.optionalAttrs (system == "x86_64-linux") {
              android = pkgsAndroid.mkShell {
                packages = with pkgsAndroid; [
                  nodejs
                  jdk21_headless
                  rustAndroidToolchain
                  rustupShim
                  androidComposition.androidsdk
                ];
                JAVA_HOME = "${pkgsAndroid.jdk21_headless.home}";
                ANDROID_HOME = androidHome;
                NDK_HOME = ndkHome;
                ANDROID_NDK_ROOT = ndkHome;
                GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${androidHome}/build-tools/37.0.0/aapt2";
                shellHook = ''
                  export CC_aarch64_linux_android="${ndkHome}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang"
                  export AR_aarch64_linux_android="${ndkHome}/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar"
                  echo "android devshell: rustup is shimmed (the fenix toolchain already ships the android targets)"
                '';
              };
            };
          };
      }
    );
}
