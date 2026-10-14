# WiimotePair

A utility app that pairs Wii Remotes to Macs running macOS 12.0 or newer.

The latest download can be found [here](https://github.com/dolphin-emu/WiimotePair/releases/latest).

## Building

### Xcode

Open `WiimotePair.xcodeproj` and build the `WiimotePair` scheme.

### Nix

WiimotePair can also be built on macOS with [Nix](https://nixos.org) without
installing Xcode:

```sh
nix build
open result/Applications/WiimotePair.app
```

For development, `nix develop` provides a shell with everything needed to run
`make`, which places the app at `build/WiimotePair.app`.

This build cannot compile `Main.storyboard` or `Assets.xcassets`, since that
requires Apple's `ibtool` and `actool`, which only ship with Xcode. Instead it
uses `nix/main.m`, which builds the same window and menu bar in code, along
with a handwritten `nix/Info.plist` and an icon generated from the PNGs in the
asset catalog. Changes to the storyboard or the Xcode project's Info.plist
settings need to be mirrored there.

The resulting app is ad-hoc signed, which is fine for local use but not for
distribution.
