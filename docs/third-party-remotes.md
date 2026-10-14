# Third-party Wii Remotes on macOS 27

Notes from debugging a third-party Wii Remote (Techken, address
`0C:FC:83:BA:94:B4`, Airoha chip) that does not pair on macOS 27.0.1
(26A434), Mac16,12, while a genuine Nintendo RVL-CNT-01 (`E8:4E:CE:...`)
does. Read this before continuing the investigation.

## How pairing works now

- IOBluetooth runs on top of CoreBluetooth. The app hooks
  `-[CBPairingAgent handlePairingRequested:]` and replies with
  `-[IOBluetoothCoreBluetoothCoordinator pairPeer:forType:withKey:]` using
  pairing type 6 (PIN code) and the PIN as a little-endian `NSNumber`
  (`src/private_api.rs`).
- The PIN is a Bluetooth address in reverse byte order: the Mac's for the
  SYNC button, the remote's own for 1+2 (`src/pairer.rs`). Retries alternate
  between the two.
- `scripts/debug-pairing.sh` (or `nix run .#debug`) runs the app while
  recording the Bluetooth logs, and puts `WiimotePair.log` and `bt.log` in a
  folder on the Desktop.

## What happens with the third-party remote

Every attempt, with either PIN, goes like this in `bt.log`:

```
Received a new pincode request for device 0C:FC:83:BA:94:B4
Sending 'pincode request' pairing event for device 0C:FC:83:BA:94:B4
handlePairingAgentRespondToPairing: Pincode (received as a number) ...
Setting pincode <private> for device 0C:FC:83:BA:94:B4
Unexpected pincode length of 6
Pairing failure reported for device 0C:FC:83:BA:94:B4    (~4 ms later)
pairingComplete result:151
```

The genuine remote gets the same sequence without "Unexpected pincode length"
and completes with result 0.

In a decompile of iOS 26.1's bluetoothd
(https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore,
`usr/sbin/bluetoothd/`), "Unexpected pincode length of %lu" is printed when
saving the PIN to the keychain fails (`BTKCSetData` returns an error), and
bluetoothd then sends a PIN code negative reply. `BTKCSetData` converts the PIN
with `CFStringCreateWithCString(pin, kCFStringEncodingUTF8)`, which fails for
bytes that are not valid UTF-8. Both PINs here are not valid UTF-8 (the SYNC
PIN fails at `0xCA`, the 1+2 PIN at `0xB4`), and the failing log has no
"BTKCSetData calling ..." lines. The keychain step only runs for some devices
(a check the decompile labels `sub_10053E610`, not decoded), and the genuine
remote skips it. This is unverified on macOS 27.

## Ruled out

- Other pairing types (0 to 8): only 6 makes bluetoothd use the PIN.
- Reply timing (immediately, or 20 ms after the request): no difference.
- The 1+2 PIN: same failure.
- `-[IOBluetoothHostController BluetoothHCIPINCodeRequestReply:inPINCodeLength:inPINCode:]`:
  returns 0 but does nothing; bluetoothd times out after 30 s (result 161).
- Dolphin's Bluetooth passthrough with the built-in Bluetooth: it needs a USB
  Bluetooth adapter that libusb can claim, and the built-in controller is not
  one.

## Experiments still open, most promising first

1. **Find what makes bluetoothd skip the keychain step for the genuine
   remote.** Compare the two remotes' Device ID (vendor ID), SDP records and
   class of device: `system_profiler SPBluetoothDataType`, the `bt.log` lines
   from `Server.Core` and `Server.SDP` (for example "Handle query complete for
   device ... pid ... isAppleBluetooth"), and an HCI capture with PacketLogger
   (in "Additional Tools for Xcode"). If the check depends on something the app
   can influence, that is the fix. If it depends on the remote's vendor ID,
   it probably is not.
2. **Confirm the keychain explanation.** In PacketLogger, check that
   bluetoothd sends HCI PIN Code Request Negative Reply (opcode 0x000E) for the
   third-party remote, and PIN Code Request Reply (0x000D) for the genuine one.
3. **Connect without pairing.** Put the remote in 1+2 mode and open L2CAP
   channels to PSM 0x11 and then 0x13 with
   `-[IOBluetoothDevice openL2CAPChannelSync:withPSM:delegate:]`, without
   starting `IOBluetoothDevicePair`. This was reported broken on macOS 12
   (https://github.com/WiiController/WiiController/issues/16), and bluetoothd
   logs "running UUID 0x1124 has security level 2 and enforcing" for these
   remotes, so it may trigger the same PIN request. If it does connect, Dolphin
   still would not see the remote (it only uses remotes macOS has connected as
   HID devices), so the app would also have to expose it, for example as a
   virtual HID device like https://github.com/gdemontalivet/wiimacmote does.
4. **Other PIN encodings.** Pass the PIN to `pairPeer:forType:withKey:` as an
   `NSString` or `NSData` instead of an `NSNumber`, in case bluetoothd stores
   those differently. bluetoothd logs "Pincode (received as a number)", which
   suggests it accepts other types. Low odds, since the keychain step converts
   the bytes to UTF-8 either way.

Genuine Wii Remotes must keep pairing on the first attempt (SYNC PIN through
the pairing agent), so test any change with one as well.
