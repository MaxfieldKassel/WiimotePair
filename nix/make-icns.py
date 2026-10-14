#!/usr/bin/env python3
# Copyright 2024 Dolphin Emulator Project
# SPDX-License-Identifier: GPL-2.0-or-later

# Packs the PNGs in an Xcode .appiconset into an .icns file without needing
# Apple's actool or iconutil.

import json
import os
import struct
import sys

# (size in points, scale) -> ICNS element type for PNG data.
ICNS_TYPES = {
    ("16x16", "1x"): b"icp4",
    ("16x16", "2x"): b"ic11",
    ("32x32", "1x"): b"icp5",
    ("32x32", "2x"): b"ic12",
    ("128x128", "1x"): b"ic07",
    ("128x128", "2x"): b"ic13",
    ("256x256", "1x"): b"ic08",
    ("256x256", "2x"): b"ic14",
    ("512x512", "1x"): b"ic09",
    ("512x512", "2x"): b"ic10",
}


def main():
    if len(sys.argv) != 3:
        sys.exit(f"usage: {sys.argv[0]} <AppIcon.appiconset> <output.icns>")

    iconset, output = sys.argv[1:]

    with open(os.path.join(iconset, "Contents.json")) as f:
        images = json.load(f)["images"]

    elements = b""
    for image in images:
        if "filename" not in image:
            continue

        icns_type = ICNS_TYPES.get((image["size"], image["scale"]))
        if icns_type is None:
            sys.exit(f"unsupported icon size: {image['size']}@{image['scale']}")

        with open(os.path.join(iconset, image["filename"]), "rb") as f:
            data = f.read()

        elements += icns_type + struct.pack(">I", len(data) + 8) + data

    with open(output, "wb") as f:
        f.write(b"icns" + struct.pack(">I", len(elements) + 8) + elements)


if __name__ == "__main__":
    main()
