#!/bin/sh
# Copyright 2024 Dolphin Emulator Project
# SPDX-License-Identifier: GPL-2.0-or-later

# Builds WiimotePair, runs it while recording the Bluetooth logs, and collects
# everything into one folder on the Desktop.
#
# PerfPowerServices and cloudpaird reply to every Bluetooth PIN code request
# without the right PIN, and bluetoothd only uses the first reply, so
# PerfPowerServices is paused (not killed) while WiimotePair runs. It is resumed
# when WiimotePair quits or this script is interrupted. Pass --no-pause to leave
# it running.
#
# Usage: scripts/debug-pairing.sh [--no-pause]

set -eu

pause=1
if [ "${1:-}" = "--no-pause" ]; then
    pause=0
fi

cd "$(dirname "$0")/.."
nix build

out="$HOME/Desktop/WiimotePair-debug-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"
app_log="$HOME/Library/Containers/org.dolphin-emu.WiimotePair/Data/Library/Logs/WiimotePair.log"

log_pid=
cleanup() {
    if [ -n "$log_pid" ]; then
        kill "$log_pid" 2>/dev/null || true
    fi
    if [ "$pause" = 1 ]; then
        sudo killall -CONT PerfPowerServices 2>/dev/null || true
        echo "Resumed PerfPowerServices."
    fi
    cp "$app_log" "$out/" 2>/dev/null || echo "Could not find $app_log"
    echo "Logs are in $out"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

if [ "$pause" = 1 ]; then
    echo "Pausing PerfPowerServices (needs your password)."
    sudo killall -STOP PerfPowerServices || echo "PerfPowerServices is not running."
fi

/usr/bin/log stream --level debug --predicate '(process == "bluetoothd" AND (category == "Server.Security" OR eventMessage CONTAINS[c] "pair" OR eventMessage CONTAINS[c] "pin")) OR (process == "WiimotePair" AND subsystem == "com.apple.bluetooth")' > "$out/bt.log" &
log_pid=$!

echo "Running WiimotePair. Press SYNC on the Wii Remote; after each failure, click OK and press SYNC again."
echo "Quit WiimotePair (Cmd-Q) when you are done."
open -W result/Applications/WiimotePair.app
