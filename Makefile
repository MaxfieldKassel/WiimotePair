# Copyright 2024 Dolphin Emulator Project
# SPDX-License-Identifier: GPL-2.0-or-later

# Builds WiimotePair.app without Xcode. Used by the Nix flake (`nix build`),
# and can be run directly inside `nix develop`.

APP_NAME := WiimotePair
BUILD_DIR ?= build
APP := $(BUILD_DIR)/$(APP_NAME).app

export MACOSX_DEPLOYMENT_TARGET ?= 12.0

PYTHON ?= python3
CODESIGN ?= rcodesign

CFLAGS ?= -O2 -Wall
ALL_CFLAGS := -fobjc-arc -fobjc-weak -IWiimotePair $(CFLAGS)
ALL_LDFLAGS := -framework Cocoa -framework CoreBluetooth -framework IOBluetooth $(LDFLAGS)

SOURCES := WiimotePair/AppDelegate.m WiimotePair/ViewController.m nix/main.m
HEADERS := $(wildcard WiimotePair/*.h)

.PHONY: all clean

all: $(APP)

$(APP): $(SOURCES) $(HEADERS) nix/Info.plist nix/make-icns.py WiimotePair/WiimotePair.entitlements WiimotePair/Credits.rtf WiimotePair/Assets.xcassets/AppIcon.appiconset/*
	@# Assemble in a temporary directory so a failed build never leaves behind
	@# a bundle that make considers up to date.
	rm -rf $@ $@.tmp
	mkdir -p $@.tmp/Contents/MacOS $@.tmp/Contents/Resources
	$(CC) $(ALL_CFLAGS) $(SOURCES) $(ALL_LDFLAGS) -o $@.tmp/Contents/MacOS/$(APP_NAME)
	cp nix/Info.plist $@.tmp/Contents/Info.plist
	printf 'APPL????' > $@.tmp/Contents/PkgInfo
	cp WiimotePair/Credits.rtf $@.tmp/Contents/Resources/
	$(PYTHON) nix/make-icns.py WiimotePair/Assets.xcassets/AppIcon.appiconset $@.tmp/Contents/Resources/AppIcon.icns
	$(CODESIGN) sign --entitlements-xml-file WiimotePair/WiimotePair.entitlements $@.tmp
	mv $@.tmp $@

clean:
	rm -rf $(BUILD_DIR)
