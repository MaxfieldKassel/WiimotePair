#!/bin/sh
# Copyright 2024 Dolphin Emulator Project
# SPDX-License-Identifier: GPL-2.0-or-later

# Builds WiimotePair, runs it while recording the Bluetooth logs, and collects
# everything into one folder on the Desktop.
#
# PerfPowerServices replies to every Bluetooth PIN code request without the
# right PIN, and bluetoothd only uses the first reply. WiimotePair usually
# replies first and retries when it does not, but pass --pause to rule this out:
# PerfPowerServices is then paused (not killed) while WiimotePair runs, and
# resumed when WiimotePair quits or this script is interrupted.
#
# Usage: scripts/debug-pairing.sh [--pause]

set -eu

pause=0
if [ "${1:-}" = "--pause" ]; then
    pause=1
fi

cd "$(dirname "$0")/.."
nix build

out="$HOME/Desktop/WiimotePair-debug-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"

log_pid=
cleanup() {
    if [ -n "$log_pid" ]; then
        kill "$log_pid" 2>/dev/null || true
    fi
    if [ "$pause" = 1 ]; then
        sudo killall -CONT PerfPowerServices 2>/dev/null || true
        echo "Resumed PerfPowerServices."
    fi
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

echo "Running WiimotePair. Press SYNC on the Wii Remote; if pairing fails, click OK and press SYNC again."
echo "Quit WiimotePair (Cmd-Q) when you are done."
# WiimotePair also writes its log to stderr, which avoids reading its sandbox container (which macOS protects).
open -W --stderr "$out/WiimotePair.log" result/Applications/WiimotePair.app
