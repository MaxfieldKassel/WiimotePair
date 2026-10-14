# WiimotePair

A utility app that pairs Wii Remotes to Macs running macOS 12.0 or newer.

The latest download can be found [here](https://github.com/dolphin-emu/WiimotePair/releases/latest).

## Building

This branch contains two implementations of WiimotePair:

- A Rust version in `src/`, built with Cargo using the
  [objc2](https://github.com/madsmtm/objc2) bindings to AppKit, CoreBluetooth
  and IOBluetooth.
- The original Objective-C version in `WiimotePair/`.

Both behave the same and produce the same app bundle.

### Nix

On macOS with [Nix](https://nixos.org), no Xcode installation is needed:

```sh
nix build            # Rust version
nix build .#objc     # Objective-C version
open result/Applications/WiimotePair.app
```

For development, `nix develop` provides a shell with everything needed to run
`make` (Rust, output in `build/WiimotePair.app`) or `make objc` (Objective-C,
output in `build/objc/WiimotePair.app`), as well as `cargo`, `clippy` and
`rustfmt`.

Neither build can compile `Main.storyboard` or `Assets.xcassets`, since that
requires Apple's `ibtool` and `actool`, which only ship with Xcode. Instead,
both create the window and menu bar in code (`src/main.rs` and `nix/main.m`),
use a handwritten `nix/Info.plist`, and generate the icon from the PNGs in the
asset catalog. Changes to the storyboard or the Xcode project's Info.plist
settings need to be mirrored there.

The resulting app is ad-hoc signed, which is fine for local use but not for
distribution.

### Xcode

The Objective-C version can also be built by opening `WiimotePair.xcodeproj`
and building the `WiimotePair` scheme.

## Troubleshooting

The Rust version writes a diagnostic log each time it runs. Since the app is
sandboxed, the log is in its container:

```sh
cat ~/Library/Containers/org.dolphin-emu.WiimotePair/Data/Library/Logs/WiimotePair.log
```

Include this log when reporting a pairing problem.
