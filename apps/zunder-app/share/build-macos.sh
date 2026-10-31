#!/usr/bin/env bash
# Builds the share extension of the macOS app and signs it:
#   build-macos.sh <version> [<signing identity>]
# Without an identity it is signed ad hoc (pull request builds; the
# extension then only runs where Gatekeeper allows unsigned code). Prints
# the path of ZunderShare.appex. Needs Xcode and xcodegen.
# Keychain: set SHARE_KEYCHAIN to the keychain holding the identity.
set -euo pipefail

version="${1:?Version fehlt}"
identity="${2:-}"
here="$(cd "$(dirname "$0")" && pwd)"
build="$here/build/macos"

rm -rf "$build"
mkdir -p "$build"
# The project lives next to the spec: its build settings (INFOPLIST_FILE,
# CODE_SIGN_ENTITLEMENTS) are relative to it.
rm -rf "$here/ZunderShare.xcodeproj"
(cd "$here" && xcodegen generate --spec project-macos.yml --quiet) >&2
xcodebuild -project "$here/ZunderShare.xcodeproj" -target ZunderShare \
  -configuration Release -sdk macosx \
  SYMROOT="$build/out" MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$version" \
  build >&2
appex="$build/out/Release/ZunderShare.appex"

args=(--force --entitlements "$here/macOS/ZunderShare.entitlements")
if [ -n "$identity" ]; then
  # Notarization needs the hardened runtime and a secure timestamp.
  args+=(--options runtime --timestamp --sign "$identity")
  [ -n "${SHARE_KEYCHAIN:-}" ] && args+=(--keychain "$SHARE_KEYCHAIN")
else
  args+=(--sign -)
fi
codesign "${args[@]}" "$appex" >&2
codesign --verify --strict "$appex" >&2
echo "$appex"
