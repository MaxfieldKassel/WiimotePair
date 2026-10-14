// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

//! Wrappers around the private IOBluetooth APIs needed to pair Wii Remotes.
//!
//! These are looked up at runtime instead of being linked against, so if a future version of macOS removes them,
//! WiimotePair can report an error instead of failing to launch or crashing.

use std::ffi::{CStr, c_uint};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObjectProtocol, Sel};
use objc2::{Message, ffi, msg_send, sel};
use objc2_foundation::NSNumber;
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
        let pairing_type: isize = msg_send![pair, currentPairingType];
        let peer: Option<Retained<AnyObject>> = msg_send![device, classicPeer];
        log!("Replying to PIN code request: pairing type {pairing_type}, classic peer {peer:?}");
        let key = NSNumber::new_u64(key);
        let _: () = msg_send![&coordinator, pairPeer: peer.as_deref(), forType: pairing_type, withKey: &*key];
    }

    Ok(())
}

/// A reply to the Wii Remote's PIN code request, prepared in advance so that it can be sent as quickly as possible.
///
/// Several system daemons, such as PerfPowerServices and cloudpaird, register pairing agents that reply to every PIN
/// code request without the right PIN, and bluetoothd only uses the first reply it receives. On macOS 27, they reply
/// within about 0.3 ms of the request, so WiimotePair has to reply before them.
struct PinReply {
    coordinator: Retained<AnyObject>,
    peer: Retained<AnyObject>,
    key: Retained<NSNumber>,
}

// SAFETY: The objects are only used to call -[IOBluetoothCoreBluetoothCoordinator pairPeer:forType:withKey:], which
// IOBluetooth itself calls from CoreBluetooth's queue.
unsafe impl Send for PinReply {}

static PIN_REPLY: Mutex<Option<PinReply>> = Mutex::new(None);

/// Whether the PIN was sent when the pairing agent received a request, before its delegate was called. Only logged.
static REPLIED: AtomicBool = AtomicBool::new(false);

/// The pairing type that CoreBluetooth reports for the Wii Remote's PIN code request on macOS 27.
const PAIRING_TYPE: isize = 0;

/// Prepares to reply to the PIN code request for `device` with `key`, until [`clear_pin_reply`] is called.
pub fn prepare_pin_reply(device: &IOBluetoothDevice, key: u64) -> Result<(), MissingApi> {
    require(device, sel!(classicPeer), "-[IOBluetoothDevice classicPeer]")?;
    // SAFETY: The method exists and returns an object.
    let peer: Option<Retained<AnyObject>> = unsafe { msg_send![device, classicPeer] };
    let peer = peer.ok_or(MissingApi("-[IOBluetoothDevice classicPeer]"))?;
    let coordinator = coordinator(None)?;
    log!("Prepared PIN reply through {coordinator:?} for {peer:?}");

    REPLIED.store(false, Ordering::SeqCst);
    *PIN_REPLY.lock().unwrap_or_else(|error| error.into_inner()) =
        Some(PinReply { coordinator, peer, key: NSNumber::new_u64(key) });
    Ok(())
}

pub fn clear_pin_reply() {
    *PIN_REPLY.lock().unwrap_or_else(|error| error.into_inner()) = None;
    REPLIED.store(false, Ordering::SeqCst);
}

/// Sends the prepared PIN reply, if there is one.
fn send_pin_reply() -> bool {
    let reply = PIN_REPLY.lock().unwrap_or_else(|error| error.into_inner());
    let Some(reply) = reply.as_ref() else {
        return false;
    };
    // SAFETY: The method exists (checked by coordinator()) and has this signature.
    let _: () =
        unsafe { msg_send![&reply.coordinator, pairPeer: &*reply.peer, forType: PAIRING_TYPE, withKey: &*reply.key] };
    true
}

/// The implementations replaced by [`install_pairing_hooks`], by class and selector.
static ORIGINAL_IMPS: Mutex<Vec<(usize, Sel, Imp)>> = Mutex::new(Vec::new());

type PeerDidRequestPairing =
    unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, isize, *mut AnyObject);
type HandlePairingRequested = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject);

/// Hooks the methods that receive the Wii Remote's PIN code request, so that WiimotePair can reply to it.
///
/// On recent versions of macOS, IOBluetooth is implemented on top of CoreBluetooth. When a classic device requests a
/// PIN code, CoreBluetooth's pairing agent receives `-handlePairingRequested:` and then calls
/// `-pairingAgent:peerDidRequestPairing:type:passkey:` on IOBluetooth, which treats the request as "Just Works"
/// pairing: it never asks IOBluetoothDevicePair's delegate for a PIN code and replies without one.
///
/// While a PIN reply is prepared, the hooks send it as early as possible: when the pairing agent receives the request,
/// or failing that, when its delegate is called.
pub fn install_pairing_hooks() {
    let hooked = hook_classes_in_image(c"CBPairingAgent", sel!(handlePairingRequested:), 3, unsafe {
        std::mem::transmute::<HandlePairingRequested, Imp>(handle_pairing_requested)
    }) + hook_classes_in_image(
        c"IOBluetoothCoreBluetoothCoordinator",
        sel!(pairingAgent:peerDidRequestPairing:type:passkey:),
        6,
        unsafe { std::mem::transmute::<PeerDidRequestPairing, Imp>(peer_did_request_pairing) },
    );
    if hooked == 0 {
        log!("Could not hook any pairing request handlers");
    }
}

/// Replaces `selector` with `hook` in every class in the same image as `image_class` that implements it itself.
///
/// `hook` must have the same signature as the method, which takes `arguments` arguments including self and _cmd.
fn hook_classes_in_image(image_class: &CStr, selector: Sel, arguments: usize, hook: Imp) -> usize {
    let Some(image_class) = AnyClass::get(image_class) else {
        log!("Not hooking {selector}: {} does not exist", image_class.to_string_lossy());
        return 0;
    };

    // SAFETY: The class is valid, and the image name is a static string or NULL.
    let image = unsafe { ffi::class_getImageName(image_class) };
    if image.is_null() {
        log!("Not hooking {selector}: could not find the image of {}", image_class.name().to_string_lossy());
        return 0;
    }

    let mut count: c_uint = 0;
    // SAFETY: image is a valid image name, and the returned array is freed below.
    let names = unsafe { ffi::objc_copyClassNamesForImage(image, &mut count) };
    if names.is_null() {
        log!(
            "Not hooking {selector}: could not list the classes in {}",
            unsafe { CStr::from_ptr(image) }.to_string_lossy()
        );
        return 0;
    }
    // SAFETY: The array contains count valid C strings.
    let classes: Vec<&'static AnyClass> = unsafe { std::slice::from_raw_parts(names, count as usize) }
        .iter()
        .filter_map(|&name| AnyClass::get(unsafe { CStr::from_ptr(name) }))
        .collect();
    // SAFETY: The array was allocated by the runtime with malloc.
    unsafe { free(names.cast()) };

    let mut hooked = 0;
    let mut original_imps = ORIGINAL_IMPS.lock().unwrap_or_else(|error| error.into_inner());
    for class in classes {
        // Only hook classes that implement the method themselves, rather than inheriting it.
        let Some(method) = class.instance_methods().iter().find(|method| method.name() == selector).copied() else {
            continue;
        };
        let class_name = class.name().to_string_lossy();
        if method.arguments_count() != arguments {
            log!("Not hooking -[{class_name} {selector}]: unexpected method signature");
            continue;
        }

        // SAFETY: The caller guarantees that the hook has the same signature as the method. The hook calls the
        // original implementation.
        let original = unsafe { method.set_implementation(hook) };
        original_imps.push((class as *const AnyClass as usize, selector, original));
        log!("Hooked -[{class_name} {selector}]");
        hooked += 1;
    }
    hooked
}

/// Finds the original implementation of `selector` for the class of `object`, or the nearest hooked superclass.
fn original_imp(object: *mut AnyObject, selector: Sel) -> Option<Imp> {
    // SAFETY: object is a valid object, since the runtime is calling a method on it.
    let object = unsafe { object.as_ref() }?;
    let original_imps = ORIGINAL_IMPS.lock().unwrap_or_else(|error| error.into_inner());
    let mut class = Some(object.class());
    while let Some(current) = class {
        let address = current as *const AnyClass as usize;
        if let Some((_, _, imp)) =
            original_imps.iter().find(|(hooked, hooked_selector, _)| *hooked == address && *hooked_selector == selector)
        {
            return Some(*imp);
        }
        class = current.superclass();
    }
    None
}

unsafe extern "C-unwind" fn handle_pairing_requested(this: *mut AnyObject, cmd: Sel, message: *mut AnyObject) {
    // Reply before doing anything else, including logging, to beat the system daemons' replies.
    let start = Instant::now();
    if send_pin_reply() {
        REPLIED.store(true, Ordering::SeqCst);
        log!("Replied to pairing request with the Wii Remote's PIN in {} µs", start.elapsed().as_micros());
    }
    // SAFETY: The message is an object, as the method's type encoding says.
    log!("Pairing agent received pairing request: {:?}", unsafe { message.as_ref() });

    match original_imp(this, cmd) {
        // SAFETY: The original implementation has this signature, and the arguments are passed through unchanged.
        Some(original) => unsafe { std::mem::transmute::<Imp, HandlePairingRequested>(original)(this, cmd, message) },
        None => log!("Could not find the original implementation of {cmd}"),
    }
}

unsafe extern "C-unwind" fn peer_did_request_pairing(
    this: *mut AnyObject,
    cmd: Sel,
    agent: *mut AnyObject,
    peer: *mut AnyObject,
    pairing_type: isize,
    passkey: *mut AnyObject,
) {
    // Send the PIN again even if it was already sent when the pairing agent received the request, in case that was a
    // different request. bluetoothd ignores replies to requests that were already answered. Never call the original
    // implementation while a reply is prepared, since it would reply without the PIN.
    let already_replied = REPLIED.swap(false, Ordering::SeqCst);
    let start = Instant::now();
    if send_pin_reply() {
        log!(
            "Replied to pairing request with the Wii Remote's PIN from the delegate in {} µs (already replied: \
             {already_replied}): type {pairing_type}, passkey {passkey:p}",
            start.elapsed().as_micros()
        );
        return;
    }

    // The passkey's type is not known for certain, so it is only logged as a pointer.
    log!("Pairing agent delegate called: type {pairing_type}, passkey {passkey:p}; no PIN reply is prepared");
    match original_imp(this, cmd) {
        // SAFETY: The original implementation has this signature, and the arguments are passed through unchanged.
        Some(original) => unsafe {
            std::mem::transmute::<Imp, PeerDidRequestPairing>(original)(this, cmd, agent, peer, pairing_type, passkey)
        },
        None => log!("Could not find the original implementation of {cmd}"),
    }
}
