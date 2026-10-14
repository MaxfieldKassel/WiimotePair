# Copyright 2024 Dolphin Emulator Project
# SPDX-License-Identifier: GPL-2.0-or-later

# Builds WiimotePair.app without Xcode. Run inside `nix develop`.
#
#   make       Builds the Rust version at build/WiimotePair.app.
#   make objc  Builds the Objective-C version at build/objc/WiimotePair.app.

BUILD_DIR ?= build

export MACOSX_DEPLOYMENT_TARGET ?= 12.0

CARGO ?= cargo
CARGO_TARGET_DIR ?= target

CFLAGS ?= -O2 -Wall
ALL_CFLAGS := -fobjc-arc -fobjc-weak -IWiimotePair $(CFLAGS)
ALL_LDFLAGS := -framework Cocoa -framework CoreBluetooth -framework IOBluetooth $(LDFLAGS)

OBJC_SOURCES := WiimotePair/AppDelegate.m WiimotePair/ViewController.m nix/main.m
OBJC_HEADERS := $(wildcard WiimotePair/*.h)

BUNDLE_INPUTS := nix/bundle.sh nix/Info.plist nix/make-icns.py WiimotePair/WiimotePair.entitlements \
	WiimotePair/Credits.rtf WiimotePair/Assets.xcassets/AppIcon.appiconset/*

.PHONY: all objc clean

all: $(BUILD_DIR)/WiimotePair.app

objc: $(BUILD_DIR)/objc/WiimotePair.app

# Cargo tracks its own dependencies, so always run it and let it decide whether anything needs rebuilding.
.PHONY: $(CARGO_TARGET_DIR)/release/WiimotePair
$(CARGO_TARGET_DIR)/release/WiimotePair:
	$(CARGO) build --release --target-dir $(CARGO_TARGET_DIR)

$(BUILD_DIR)/WiimotePair.app: $(CARGO_TARGET_DIR)/release/WiimotePair $(BUNDLE_INPUTS)
	sh nix/bundle.sh $< $@

$(BUILD_DIR)/objc/WiimotePair: $(OBJC_SOURCES) $(OBJC_HEADERS)
	mkdir -p $(@D)
	$(CC) $(ALL_CFLAGS) $(OBJC_SOURCES) $(ALL_LDFLAGS) -o $@

$(BUILD_DIR)/objc/WiimotePair.app: $(BUILD_DIR)/objc/WiimotePair $(BUNDLE_INPUTS)
	sh nix/bundle.sh $< $@

clean:
	rm -rf $(BUILD_DIR)
	$(CARGO) clean --target-dir $(CARGO_TARGET_DIR)
