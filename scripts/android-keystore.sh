#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
#
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Create the release keystore the Android build signs with, and write the
# `keystore.properties` that app/build.gradle.kts reads. Run it once per machine;
# the file it writes is git-ignored, and losing it means a new signing identity
# (Play Store will refuse an update signed with another key).
#
#   scripts/android-keystore.sh                 # asks for the passwords
#   STORE_PASSWORD=… KEY_PASSWORD=… scripts/android-keystore.sh
#
# The same four names can be passed as environment variables to a build instead
# of writing the file: STORE_FILE, STORE_PASSWORD, KEY_ALIAS, KEY_PASSWORD.

set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
target="$root/src-tauri/gen/android/app"
keystore="$target/upload.jks"
properties="$target/keystore.properties"
alias_name="${KEY_ALIAS:-upload}"

if [ -f "$keystore" ]; then
  echo "a keystore already exists: $keystore" >&2
  echo "delete it first if you really mean to replace the signing identity" >&2
  exit 1
fi

store_password="${STORE_PASSWORD:-}"
key_password="${KEY_PASSWORD:-}"
if [ -z "$store_password" ]; then
  read -r -s -p "keystore password: " store_password
  echo
fi
if [ -z "$key_password" ]; then
  key_password="$store_password"
fi

dname="${KEY_DNAME:-CN=ledger-app, OU=Android, O=Alper Celik, L=Istanbul, C=TR}"

keytool -genkeypair -v \
  -keystore "$keystore" \
  -alias "$alias_name" \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -storepass "$store_password" -keypass "$key_password" \
  -dname "$dname" >/dev/null

cat > "$properties" <<PROPERTIES
# Written by scripts/android-keystore.sh — git-ignored, never commit it.
storeFile=$keystore
storePassword=$store_password
keyAlias=$alias_name
keyPassword=$key_password
PROPERTIES
chmod 600 "$properties" "$keystore"

echo "wrote $keystore"
echo "wrote $properties"
echo
echo "back the keystore up somewhere safe: the same key must sign every update."
