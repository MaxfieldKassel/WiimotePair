// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

//! Wrappers around the private IOBluetooth APIs needed to pair Wii Remotes.
//!
//! These are looked up at runtime instead of being linked against, so if a future version of macOS removes them,
//! WiimotePair can report an error instead of failing to launch or crashing.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol, Sel};
use objc2::{Message, msg_send, sel};
use objc2_foundation::{NSNumber, NSUInteger};
use objc2_io_bluetooth::{IOBluetoothDevice, IOBluetoothDevicePair};

#[derive(Debug)]
pub struct MissingApi(&'static str);

impl std::fmt::Display for MissingApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "This version of macOS is missing a private API that WiimotePair requires ({}).", self.0)
    }
}

fn require(object: &(impl NSObjectProtocol + Message), selector: Sel, name: &'static str) -> Result<(), MissingApi> {
    if object.respondsToSelector(selector) { Ok(()) } else { Err(MissingApi(name)) }
}

/// Makes the pair always ask its delegate for a PIN code, which is needed to supply the Wii Remote's PIN.
pub fn set_user_defined_pincode(pair: &IOBluetoothDevicePair, enabled: bool) -> Result<(), MissingApi> {
    require(pair, sel!(setUserDefinedPincode:), "-[IOBluetoothDevicePair setUserDefinedPincode:]")?;
    // SAFETY: The method exists and takes a BOOL.
    unsafe { msg_send![pair, setUserDefinedPincode: enabled] }
    Ok(())
}

/// Replies to a PIN code request with the given key.
///
/// This is what `-[IOBluetoothDevicePair replyPINCode:PINCode:]` essentially does. However, that method does a bunch
/// of NSString-ification on the PIN code first. We don't want this, so we replicate its behaviour here while skipping
/// the NSString stuff.
pub fn reply_pin_code(pair: &IOBluetoothDevicePair, device: &IOBluetoothDevice, key: u64) -> Result<(), MissingApi> {
    require(pair, sel!(currentPairingType), "-[IOBluetoothDevicePair currentPairingType]")?;
    require(device, sel!(classicPeer), "-[IOBluetoothDevice classicPeer]")?;

    let coordinator_class = AnyClass::get(c"IOBluetoothCoreBluetoothCoordinator")
        .ok_or(MissingApi("IOBluetoothCoreBluetoothCoordinator"))?;
    // SAFETY: The class method exists and returns an object.
    let coordinator: Option<Retained<AnyObject>> = unsafe {
        if coordinator_class.responds_to(sel!(sharedInstance)) {
            msg_send![coordinator_class, sharedInstance]
        } else {
            None
        }
    };
    let coordinator = coordinator.ok_or(MissingApi("+[IOBluetoothCoreBluetoothCoordinator sharedInstance]"))?;
    if !coordinator.class().responds_to(sel!(pairPeer:forType:withKey:)) {
        return Err(MissingApi("-[IOBluetoothCoreBluetoothCoordinator pairPeer:forType:withKey:]"));
    }

    // SAFETY: These methods exist (checked above) and have these signatures.
    unsafe {
        let pairing_type: NSUInteger = msg_send![pair, currentPairingType];
        let peer: Option<Retained<AnyObject>> = msg_send![device, classicPeer];
        let key = NSNumber::new_u64(key);
        let _: () = msg_send![&coordinator, pairPeer: peer.as_deref(), forType: pairing_type, withKey: &*key];
    }

    Ok(())
}
