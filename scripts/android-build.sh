#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
#
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build the Android app inside the flake's phone shell. Everything after the
# script name is handed to `tauri android build` unchanged, so:
#
#   scripts/android-build.sh                                   # debug APK, all 4 ABIs
#   scripts/android-build.sh --apk --debug --target aarch64    # one ABI (much faster)
#   scripts/android-build.sh --apk --release                   # stripped APK + AAB
#
# `nix develop .#android` supplies the SDK, NDK, JDK and a Rust toolchain whose
# sysroot carries the Android standard libraries; the shell hook points
# ANDROID_HOME/NDK_HOME/JAVA_HOME at them, keeps Gradle's caches in
# `.gradle-android`, and puts the NDK clang wrappers on PATH for
# `src-tauri/.cargo/config.toml`. To keep the desktop builds' `src-tauri/target`
# untouched, prefix with `CARGO_TARGET_DIR=$PWD/src-tauri/target/android`.

set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

# A debug APK when called with no arguments; otherwise the caller decides
# (passing `--apk`/`--aab`/`--release`/`--target` exactly as the CLI documents).
if [ "$#" -eq 0 ]; then
  set -- --apk --debug
fi

exec nix develop .#android --command npm run tauri android build -- "$@"
