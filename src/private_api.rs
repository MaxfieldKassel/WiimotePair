// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

//! Wrappers around the private IOBluetooth APIs needed to pair Wii Remotes.
//!
//! These are looked up at runtime instead of being linked against, so if a future version of macOS removes them,
//! WiimotePair can report an error instead of failing to launch or crashing.

use std::ffi::{CStr, c_int, c_uint};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Imp, NSObjectProtocol, Sel};
use objc2::{Message, ffi, msg_send, sel};
use objc2_foundation::{NSNumber, NSString};
use objc2_io_bluetooth::{
    BluetoothDeviceAddress, BluetoothPINCode, IOBluetoothDevice, IOBluetoothDevicePair, IOBluetoothHostController,
    IOBluetoothNSStringToDeviceAddress,
};

use crate::errors::describe_io_return;
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

/// How a PIN reply is sent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReplyPath {
    /// Through CoreBluetooth's pairing agent, with `-[IOBluetoothCoreBluetoothCoordinator pairPeer:forType:withKey:]`.
    PairingAgent,
    /// Straight to the Bluetooth controller, with `-[IOBluetoothHostController
    /// BluetoothHCIPINCodeRequestReply:inPINCodeLength:inPINCode:]`.
    ///
    /// This is an experiment for third-party Wii Remotes. For some devices, bluetoothd saves a PIN received from a
    /// pairing agent to the keychain as a UTF-8 string before using it. Wii Remote PINs are binary and usually not valid
    /// UTF-8, so saving fails, bluetoothd logs "Unexpected pincode length of 6" and refuses the PIN code request. Replying
    /// to the controller directly may avoid that, if macOS still allows it.
    Controller,
}

/// A reply to the Wii Remote's PIN code request, prepared before pairing starts.
struct PinReply {
    path: ReplyPath,
    coordinator: Retained<AnyObject>,
    peer: Retained<AnyObject>,
    key: Retained<NSNumber>,
    controller: Retained<IOBluetoothHostController>,
    device_address: BluetoothDeviceAddress,
    pin: BluetoothPINCode,
}

// SAFETY: The objects are only used to send the reply, which IOBluetooth itself does from CoreBluetooth's queue.
unsafe impl Send for PinReply {}

/// The length of a Wii Remote PIN, which is a Bluetooth address.
const PIN_LENGTH: usize = 6;

static PIN_REPLY: Mutex<Option<PinReply>> = Mutex::new(None);

/// Whether the pairing agent hook has handled the current request, so that its delegate does not reply again.
static HANDLED: AtomicBool = AtomicBool::new(false);

/// How long to wait before replying to a request that says it is for PIN code pairing.
///
/// bluetoothd sends that request (`kCBMsgArgPairingType` = 6) only to the app that started pairing, about 1 ms before it
/// logs "Sending 'pincode request' pairing event". In the attempt that paired a genuine Wii Remote, the reply arrived
/// after that, so replies wait until then. bluetoothd waits several seconds for the reply.
const PIN_REPLY_DELAY: Duration = Duration::from_millis(20);

/// The pairing type that bluetoothd uses for PIN code (legacy) pairing.
///
/// CoreBluetooth passes it to pairing agents as `kCBMsgArgPairingType`. When a reply uses any other type, such as the
/// type 0 ("Just Works") that IOBluetooth replies with, bluetoothd ignores the PIN and pairing times out.
const PIN_CODE_PAIRING_TYPE: isize = 6;

/// Prepares to reply to the PIN code request for `device` with `key` through `path`, until [`clear_pin_reply`] is
/// called. `key` holds the PIN bytes in little-endian order.
pub fn prepare_pin_reply(device: &IOBluetoothDevice, key: u64, path: ReplyPath) -> Result<(), MissingApi> {
    require(device, sel!(classicPeer), "-[IOBluetoothDevice classicPeer]")?;
    // SAFETY: The method exists and returns an object.
    let peer: Option<Retained<AnyObject>> = unsafe { msg_send![device, classicPeer] };
    let peer = peer.ok_or(MissingApi("-[IOBluetoothDevice classicPeer]"))?;
    let coordinator = coordinator(None)?;

    let controller = unsafe { IOBluetoothHostController::defaultController() }
        .ok_or(MissingApi("+[IOBluetoothHostController defaultController]"))?;
    let path = if path == ReplyPath::Controller
        && !controller.respondsToSelector(sel!(BluetoothHCIPINCodeRequestReply:inPINCodeLength:inPINCode:))
    {
        log!("This version of macOS cannot send PIN code replies to the Bluetooth controller; using the pairing agent");
        ReplyPath::PairingAgent
    } else {
        path
    };

    let mut device_address = BluetoothDeviceAddress { data: [0; 6] };
    if let Some(address_string) = unsafe { device.addressString() } {
        // SAFETY: device_address is a valid pointer to a BluetoothDeviceAddress.
        unsafe { IOBluetoothNSStringToDeviceAddress(Some(&address_string), &mut device_address) };
    }

    let mut pin = BluetoothPINCode { data: [0; 16] };
    pin.data[..PIN_LENGTH].copy_from_slice(&key.to_le_bytes()[..PIN_LENGTH]);

    log!("Prepared PIN reply through {path:?} for {peer:?} ({:02X?})", device_address.data);

    HANDLED.store(false, Ordering::SeqCst);
    *PIN_REPLY.lock().unwrap_or_else(|error| error.into_inner()) =
        Some(PinReply { path, coordinator, peer, key: NSNumber::new_u64(key), controller, device_address, pin });
    Ok(())
}

pub fn clear_pin_reply() {
    *PIN_REPLY.lock().unwrap_or_else(|error| error.into_inner()) = None;
    HANDLED.store(false, Ordering::SeqCst);
}

fn has_pin_reply() -> bool {
    PIN_REPLY.lock().unwrap_or_else(|error| error.into_inner()).is_some()
}

/// Sends the prepared PIN reply, if there is one.
fn send_pin_reply() -> bool {
    let reply = PIN_REPLY.lock().unwrap_or_else(|error| error.into_inner());
    let Some(reply) = reply.as_ref() else {
        return false;
    };
    match reply.path {
        ReplyPath::PairingAgent => {
            // SAFETY: The method exists (checked by coordinator()) and has this signature.
            let _: () = unsafe {
                msg_send![&reply.coordinator, pairPeer: &*reply.peer, forType: PIN_CODE_PAIRING_TYPE, withKey: &*reply.key]
            };
        }
        ReplyPath::Controller => {
            // SAFETY: The method exists (checked by prepare_pin_reply()) and has this signature, according to the macOS
            // 26.4 IOBluetooth headers. The pointers are valid for the duration of the call.
            let result: c_int = unsafe {
                msg_send![
                    &*reply.controller,
                    BluetoothHCIPINCodeRequestReply: &reply.device_address as *const BluetoothDeviceAddress,
                    inPINCodeLength: PIN_LENGTH as u64,
                    inPINCode: &reply.pin as *const BluetoothPINCode
                ]
            };
            log!("Bluetooth controller PIN code reply returned {result:#x} ({})", describe_io_return(result));
        }
    }
    true
}

/// Replies to a pairing request of the given type with the prepared PIN.
///
/// Requests for PIN code pairing are answered after [`PIN_REPLY_DELAY`]. Other requests, which bluetoothd sends to
/// every pairing agent, are answered immediately: system daemons such as PerfPowerServices reply to those without the
/// right PIN within about 0.3 ms, and bluetoothd only uses the first reply.
fn reply_to_request(pairing_type: Option<isize>, source: &'static str) {
    if pairing_type == Some(PIN_CODE_PAIRING_TYPE) {
        std::thread::spawn(move || {
            std::thread::sleep(PIN_REPLY_DELAY);
            if send_pin_reply() {
                log!("Replied to PIN code pairing request from the {source} after {} ms", PIN_REPLY_DELAY.as_millis());
            }
        });
    } else {
        let start = Instant::now();
        if send_pin_reply() {
            log!(
                "Replied to pairing request of type {pairing_type:?} from the {source} in {} µs",
                start.elapsed().as_micros()
            );
        }
    }
}

/// Returns `kCBMsgArgPairingType` from a pairing request that CoreBluetooth received from bluetoothd.
fn request_pairing_type(message: &AnyObject) -> Option<isize> {
    if !message.class().responds_to(sel!(objectForKey:)) {
        return None;
    }
    // SAFETY: The message is a dictionary, which responds to objectForKey: with an object or nil.
    let value: Option<Retained<AnyObject>> =
        unsafe { msg_send![message, objectForKey: objc2_foundation::ns_string!("kCBMsgArgPairingType")] };
    let value = value.filter(|value| value.class().responds_to(sel!(integerValue)))?;
    // SAFETY: The value responds to integerValue, which returns an NSInteger.
    Some(unsafe { msg_send![&value, integerValue] })
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
/// `-pairingAgent:peerDidRequestPairing:type:passkey:` on IOBluetooth, which replies without the PIN and never asks
/// IOBluetoothDevicePair's delegate for one.
///
/// While a PIN reply is prepared, the hooks reply with it when the pairing agent receives the request, or failing that,
/// when its delegate is called. See [`reply_to_request`] for when.
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

/// An object that is only read, and so can be sent to the logging thread.
struct SendObject(Retained<AnyObject>);

// SAFETY: Only used for immutable objects, such as the dictionaries that CoreBluetooth passes to pairing agents.
unsafe impl Send for SendObject {}

unsafe extern "C-unwind" fn handle_pairing_requested(this: *mut AnyObject, cmd: Sel, message: *mut AnyObject) {
    // Handle the request before doing anything else, since some requests must be answered as quickly as possible.
    // Logging happens on another thread.
    if has_pin_reply() {
        // SAFETY: The message is an object, as the method's type encoding says.
        let pairing_type = unsafe { message.as_ref() }.and_then(request_pairing_type);
        HANDLED.store(true, Ordering::SeqCst);
        reply_to_request(pairing_type, "pairing agent");
    }

    match original_imp(this, cmd) {
        // SAFETY: The original implementation has this signature, and the arguments are passed through unchanged.
        Some(original) => unsafe { std::mem::transmute::<Imp, HandlePairingRequested>(original)(this, cmd, message) },
        None => log!("Could not find the original implementation of {cmd}"),
    }

    // Describe the request on another thread, since it is slow. It shows what bluetoothd sent, including the pairing
    // type before CoreBluetooth converts it.
    // SAFETY: The message is an object, as the method's type encoding says.
    if let Some(message) = unsafe { message.as_ref() } {
        let message = SendObject(message.retain());
        std::thread::spawn(move || {
            let message = message;
            autoreleasepool(|_| {
                // SAFETY: Every object responds to description, which returns a string.
                let description: Retained<NSString> = unsafe { msg_send![&*message.0, description] };
                log!("Pairing agent received pairing request: {description}");
            });
        });
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
    if HANDLED.swap(false, Ordering::SeqCst) {
        // Calling the original implementation would reply again, without the PIN.
        log!("Pairing agent delegate called: type {pairing_type}, passkey {passkey:p}; already handled");
        return;
    }

    if has_pin_reply() {
        let pairing_type = if pairing_type == 0 { None } else { Some(pairing_type) };
        reply_to_request(pairing_type, "pairing agent delegate");
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
