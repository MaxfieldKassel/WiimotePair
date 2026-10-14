#!/bin/sh
# Copyright 2024 Dolphin Emulator Project
# SPDX-License-Identifier: GPL-2.0-or-later

# Assembles and ad-hoc signs WiimotePair.app around an already built executable.
#
# Usage: nix/bundle.sh <executable> <output .app>

set -eu

if [ $# -ne 2 ]; then
    echo "usage: $0 <executable> <output .app>" >&2
    exit 1
fi

executable=$1
app=$2
root=$(dirname "$0")/..

: "${PYTHON:=python3}"
: "${CODESIGN:=rcodesign}"

# Assemble in a temporary directory so a failed build never leaves behind a
# bundle that make considers up to date.
rm -rf "$app" "$app.tmp"
mkdir -p "$app.tmp/Contents/MacOS" "$app.tmp/Contents/Resources"

cp "$executable" "$app.tmp/Contents/MacOS/WiimotePair"
cp "$root/nix/Info.plist" "$app.tmp/Contents/Info.plist"
printf 'APPL????' > "$app.tmp/Contents/PkgInfo"
cp "$root/WiimotePair/Credits.rtf" "$app.tmp/Contents/Resources/"
"$PYTHON" "$root/nix/make-icns.py" "$root/WiimotePair/Assets.xcassets/AppIcon.appiconset" "$app.tmp/Contents/Resources/AppIcon.icns"

$CODESIGN sign --entitlements-xml-file "$root/WiimotePair/WiimotePair.entitlements" "$app.tmp"

mv "$app.tmp" "$app"
