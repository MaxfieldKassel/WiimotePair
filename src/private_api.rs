// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

//! Wrappers around the private IOBluetooth APIs needed to pair Wii Remotes.
//!
//! These are looked up at runtime instead of being linked against, so if a future version of macOS removes them,
//! WiimotePair can report an error instead of failing to launch or crashing.

use std::ffi::{CStr, c_uint};
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObjectProtocol, Sel};
use objc2::{Message, ffi, msg_send, sel};
use objc2_foundation::{NSNumber, NSUInteger};
use objc2_io_bluetooth::{IOBluetoothDevice, IOBluetoothDevicePair};

use crate::log::log;

unsafe extern "C" {
    fn free(pointer: *mut std::ffi::c_void);
}

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

/// Class methods that may return the shared IOBluetoothCoreBluetoothCoordinator. Its name differs between versions of
/// macOS: macOS 27 does not have `sharedInstance`.
const COORDINATOR_ACCESSORS: &[&CStr] =
    &[c"sharedInstance", c"sharedCoordinator", c"defaultCoordinator", c"sharedManager", c"shared", c"coordinator"];

/// Finds the IOBluetoothCoreBluetoothCoordinator that can reply to pairing requests.
///
/// `receiver` is the object that received a pairing request, which may be the coordinator itself or may know it.
fn coordinator(receiver: Option<&AnyObject>) -> Result<Retained<AnyObject>, MissingApi> {
    let can_pair = |object: &AnyObject| object.class().responds_to(sel!(pairPeer:forType:withKey:));

    if let Some(receiver) = receiver {
        if can_pair(receiver) {
            return Ok(receiver.retain());
        }
        if receiver.class().responds_to(sel!(coordinator)) {
            // SAFETY: The method exists, and is assumed to be a getter that returns an object.
            let coordinator: Option<Retained<AnyObject>> = unsafe { msg_send![receiver, coordinator] };
            if let Some(coordinator) = coordinator.filter(|coordinator| can_pair(coordinator)) {
                return Ok(coordinator);
            }
        }
    }

    let coordinator_class = AnyClass::get(c"IOBluetoothCoreBluetoothCoordinator")
        .ok_or(MissingApi("IOBluetoothCoreBluetoothCoordinator"))?;
    for name in COORDINATOR_ACCESSORS {
        let selector = Sel::register(name);
        // Only call getters: methods that take no arguments and return an object.
        let Some(method) = coordinator_class.class_method(selector) else {
            continue;
        };
        if method.arguments_count() != 2 || method.return_type().to_bytes() != b"@" {
            continue;
        }
        // SAFETY: The class method exists, takes no arguments and returns an object.
        let coordinator: Option<Retained<AnyObject>> =
            unsafe { msg_send![coordinator_class, performSelector: selector] };
        if let Some(coordinator) = coordinator.filter(|coordinator| can_pair(coordinator)) {
            return Ok(coordinator);
        }
    }

    Err(MissingApi("-[IOBluetoothCoreBluetoothCoordinator pairPeer:forType:withKey:]"))
}

/// Logs the methods of a class that are related to pairing, to help find replacements for private APIs that change.
fn log_methods(class_name: &CStr) {
    let Some(class) = AnyClass::get(class_name) else {
        log!("{} does not exist", class_name.to_string_lossy());
        return;
    };
    let keywords = ["pair", "pin", "shared", "coordinator", "respond", "reply", "agent", "passkey", "instance"];
    for (prefix, class) in [("+", class.metaclass()), ("-", class)] {
        for method in class.instance_methods().iter() {
            let name = method.name().name().to_string_lossy();
            if !keywords.iter().any(|keyword| name.to_lowercase().contains(keyword)) {
                continue;
            }
            // SAFETY: The method is valid, and the encoding is a static string or NULL.
            let encoding = unsafe { ffi::method_getTypeEncoding(*method) };
            let encoding =
                if encoding.is_null() { "?".into() } else { unsafe { CStr::from_ptr(encoding) }.to_string_lossy() };
            log!("  {prefix}[{} {name}] {encoding}", class_name.to_string_lossy());
        }
    }
}

/// Replies to a PIN code request with the given key.
///
/// This is what `-[IOBluetoothDevicePair replyPINCode:PINCode:]` essentially does. However, that method does a bunch
/// of NSString-ification on the PIN code first. We don't want this, so we replicate its behaviour here while skipping
/// the NSString stuff.
pub fn reply_pin_code(pair: &IOBluetoothDevicePair, device: &IOBluetoothDevice, key: u64) -> Result<(), MissingApi> {
    require(pair, sel!(currentPairingType), "-[IOBluetoothDevicePair currentPairingType]")?;
    require(device, sel!(classicPeer), "-[IOBluetoothDevice classicPeer]")?;

    let coordinator = coordinator(None)?;

    // SAFETY: These methods exist (checked above) and have these signatures.
    unsafe {
        let pairing_type: NSUInteger = msg_send![pair, currentPairingType];
        let peer: Option<Retained<AnyObject>> = msg_send![device, classicPeer];
        log!("Replying to PIN code request: pairing type {pairing_type}, classic peer {peer:?}");
        let key = NSNumber::new_u64(key);
        let _: () = msg_send![&coordinator, pairPeer: peer.as_deref(), forType: pairing_type, withKey: &*key];
    }

    Ok(())
}

/// The key to reply with when a pairing agent requests pairing, set while WiimotePair is pairing a Wii Remote.
static PAIRING_KEY: Mutex<Option<u64>> = Mutex::new(None);

/// The implementations of `pairingAgent:peerDidRequestPairing:type:passkey:` replaced by [`install_pairing_hook`].
static ORIGINAL_IMPS: Mutex<Vec<(usize, Imp)>> = Mutex::new(Vec::new());

type PeerDidRequestPairing =
    unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, isize, *mut AnyObject);

/// Sets the key that the pairing hook replies with, or clears it.
pub fn set_pairing_key(key: Option<u64>) {
    *PAIRING_KEY.lock().unwrap_or_else(|error| error.into_inner()) = key;
}

/// Hooks the CoreBluetooth pairing agent delegate methods that IOBluetooth implements.
///
/// On recent versions of macOS, IOBluetooth is implemented on top of CoreBluetooth. When a classic device requests a
/// PIN code, CoreBluetooth calls `-pairingAgent:peerDidRequestPairing:type:passkey:` on IOBluetooth, which treats the
/// request as "Just Works" pairing: it never asks IOBluetoothDevicePair's delegate for a PIN code and replies without
/// one, so pairing a Wii Remote fails. While a pairing key is set, the hook replies with that key instead.
pub fn install_pairing_hook() {
    let selector = sel!(pairingAgent:peerDidRequestPairing:type:passkey:);

    let Some(coordinator_class) = AnyClass::get(c"IOBluetoothCoreBluetoothCoordinator") else {
        log!("Not hooking pairing requests: IOBluetoothCoreBluetoothCoordinator does not exist");
        return;
    };

    // The method may be implemented by a class other than the coordinator, so check every class in its image.
    // SAFETY: The class is valid, and the image name is a static string or NULL.
    let image = unsafe { ffi::class_getImageName(coordinator_class) };
    if image.is_null() {
        log!("Not hooking pairing requests: could not find the IOBluetooth image");
        return;
    }

    let mut count: c_uint = 0;
    // SAFETY: image is a valid image name, and the returned array is freed below.
    let names = unsafe { ffi::objc_copyClassNamesForImage(image, &mut count) };
    if names.is_null() {
        log!("Not hooking pairing requests: could not list the IOBluetooth classes");
        return;
    }
    // SAFETY: The array contains count valid C strings.
    let classes: Vec<&'static AnyClass> = unsafe { std::slice::from_raw_parts(names, count as usize) }
        .iter()
        .filter_map(|&name| AnyClass::get(unsafe { CStr::from_ptr(name) }))
        .collect();
    // SAFETY: The array was allocated by the runtime with malloc.
    unsafe { free(names.cast()) };

    let mut original_imps = ORIGINAL_IMPS.lock().unwrap_or_else(|error| error.into_inner());
    for class in classes {
        // Only hook classes that implement the method themselves, rather than inheriting it.
        let Some(method) = class.instance_methods().iter().find(|method| method.name() == selector).copied() else {
            continue;
        };
        if method.arguments_count() != 6 {
            log!("Not hooking pairing requests in {}: unexpected method signature", class.name().to_string_lossy());
            continue;
        }

        let hook: PeerDidRequestPairing = peer_did_request_pairing;
        // SAFETY: The hook has the same signature as the original method, and calls it when it does not handle the
        // request.
        let original = unsafe { method.set_implementation(std::mem::transmute::<PeerDidRequestPairing, Imp>(hook)) };
        original_imps.push((class as *const AnyClass as usize, original));
        log!("Hooked pairing requests in {}", class.name().to_string_lossy());
    }

    if original_imps.is_empty() {
        log!("Not hooking pairing requests: no class implements {selector}");
    }
    drop(original_imps);

    log!("Pairing related methods:");
    for class_name in [c"IOBluetoothCoreBluetoothCoordinator", c"IOBluetoothDevicePair", c"CBPairingAgent"] {
        log_methods(class_name);
    }
}

/// Finds the original implementation for the class of `object`, or the nearest hooked superclass.
fn original_imp(object: &AnyObject) -> Option<PeerDidRequestPairing> {
    let original_imps = ORIGINAL_IMPS.lock().unwrap_or_else(|error| error.into_inner());
    let mut class = Some(object.class());
    while let Some(current) = class {
        if let Some((_, imp)) = original_imps.iter().find(|(hooked, _)| *hooked == current as *const AnyClass as usize)
        {
            // SAFETY: The original implementation has this signature.
            return Some(unsafe { std::mem::transmute::<Imp, PeerDidRequestPairing>(*imp) });
        }
        class = current.superclass();
    }
    None
}

unsafe extern "C-unwind" fn peer_did_request_pairing(
    this: *mut AnyObject,
    cmd: Sel,
    agent: *mut AnyObject,
    peer: *mut AnyObject,
    pairing_type: isize,
    passkey: *mut AnyObject,
) {
    // The passkey's type is not known for certain, so it is only logged as a pointer.
    // SAFETY: this is a valid object, since the runtime is calling a method on it.
    let class = unsafe { this.as_ref() }.map(|this| this.class().name().to_string_lossy()).unwrap_or_default();
    log!("Pairing agent requested pairing from {class}: type {pairing_type}, passkey {passkey:p}, peer {peer:p}");

    let key = *PAIRING_KEY.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(key) = key
        && !peer.is_null()
    {
        // SAFETY: this is a valid object, since the runtime is calling a method on it.
        match coordinator(unsafe { this.as_ref() }) {
            Ok(coordinator) => {
                log!("Replying to pairing request with the Wii Remote's PIN through {coordinator:?}");
                let key = NSNumber::new_u64(key);
                // SAFETY: The method exists (checked by coordinator()), and peer is the peer that requested pairing.
                let _: () = unsafe {
                    msg_send![&coordinator, pairPeer: peer, forType: pairing_type as NSUInteger, withKey: &*key]
                };
                return;
            }
            Err(error) => log!("Could not reply to pairing request: {error}"),
        }
    }

    // SAFETY: this is a valid object, since the runtime is calling a method on it.
    let original = unsafe { this.as_ref() }.and_then(original_imp);
    match original {
        // SAFETY: The arguments are passed through unchanged.
        Some(original) => unsafe { original(this, cmd, agent, peer, pairing_type, passkey) },
        None => log!("Could not find the original pairing request handler"),
    }
}
