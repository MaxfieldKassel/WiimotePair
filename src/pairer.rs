// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::c_int;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSWindow};
use objc2_core_bluetooth::{CBCentralManager, CBCentralManagerDelegate, CBManagerState};
use objc2_foundation::{NSObject, NSObjectProtocol};
use objc2_io_bluetooth::{
    BluetoothDeviceAddress, IOBluetoothDevice, IOBluetoothDeviceInquiry, IOBluetoothDeviceInquiryDelegate,
    IOBluetoothDevicePair, IOBluetoothDevicePairDelegate, IOBluetoothDeviceSearchTypesBits, IOBluetoothHostController,
    IOBluetoothNSStringToDeviceAddress,
};

use crate::errors::{describe_io_return, describe_pairing_error};
use crate::log::log;
use crate::private_api;
use crate::show_alert;

const K_IO_RETURN_SUCCESS: c_int = 0;

/// How many times to retry pairing a Wii Remote without telling the user.
///
/// Pairing sometimes fails for reasons outside WiimotePair's control, for example when a system daemon such as
/// PerfPowerServices replies to the Wii Remote's PIN code request first, without the right PIN. Retries also try the PIN
/// for the other pairing button (see [`PairingButton`]). A failed attempt is worth retrying while the Wii Remote is still
/// in pairing mode.
const AUTOMATIC_RETRIES: u32 = 3;

pub struct PairerIvars {
    window: Retained<NSWindow>,
    central_manager: OnceCell<Retained<CBCentralManager>>,
    device_inquiry: RefCell<Option<Retained<IOBluetoothDeviceInquiry>>>,
    device_pair: RefCell<Option<Retained<IOBluetoothDevicePair>>>,
    failed_attempts: Cell<u32>,
    showing_result: Cell<bool>,
}

define_class!(
    // SAFETY:
    // - NSObject does not have any subclassing requirements.
    // - Pairer does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PairerIvars]
    pub struct Pairer;

    unsafe impl NSObjectProtocol for Pairer {}

    unsafe impl CBCentralManagerDelegate for Pairer {
        #[unsafe(method(centralManagerDidUpdateState:))]
        fn central_manager_did_update_state(&self, central_manager: &CBCentralManager) {
            let state = unsafe { central_manager.state() };
            log!("Bluetooth state changed to {}", state.0);
            self.handle_state(state);
        }
    }

    unsafe impl IOBluetoothDeviceInquiryDelegate for Pairer {
        #[unsafe(method(deviceInquiryDeviceFound:device:))]
        fn device_inquiry_device_found(
            &self,
            sender: Option<&IOBluetoothDeviceInquiry>,
            device: Option<&IOBluetoothDevice>,
        ) {
            if let (Some(sender), Some(device)) = (sender, device) {
                self.handle_device_found(sender, device);
            }
        }

        #[unsafe(method(deviceInquiryComplete:error:aborted:))]
        fn device_inquiry_complete(&self, sender: Option<&IOBluetoothDeviceInquiry>, error: c_int, aborted: bool) {
            log!("Inquiry complete: error {error:#x}, aborted {aborted}");
            // Restart inquiries that have timed out, unless a Wii Remote is being paired or the result of pairing one
            // is being shown. Stopping an inquiry to pair a Wii Remote also reports it as complete without aborting.
            if let (Some(sender), false, false) = (sender, aborted, self.is_busy()) {
                unsafe {
                    sender.clearFoundDevices();
                    sender.start();
                }
            }
        }
    }

    unsafe impl IOBluetoothDevicePairDelegate for Pairer {
        #[unsafe(method(devicePairingPINCodeRequest:))]
        fn device_pairing_pin_code_request(&self, sender: Option<&AnyObject>) {
            if let Some(pair) = sender.and_then(|sender| sender.downcast_ref::<IOBluetoothDevicePair>()) {
                self.handle_pin_code_request(pair);
            }
        }

        #[unsafe(method(devicePairingFinished:error:))]
        fn device_pairing_finished(&self, _sender: Option<&AnyObject>, error: c_int) {
            self.handle_pairing_finished(error);
        }
    }
);

impl Pairer {
    pub fn new(window: &NSWindow, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PairerIvars {
            window: window.retain(),
            central_manager: OnceCell::new(),
            device_inquiry: RefCell::new(None),
            device_pair: RefCell::new(None),
            failed_attempts: Cell::new(0),
            showing_result: Cell::new(false),
        });
        // SAFETY: NSObject's init method has this signature.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // SAFETY: A nil queue delivers delegate callbacks on the main queue, matching Pairer's thread requirements.
        let central_manager = unsafe {
            CBCentralManager::initWithDelegate_queue(
                CBCentralManager::alloc(),
                Some(ProtocolObject::from_ref(&*this)),
                None,
            )
        };
        this.ivars().central_manager.set(central_manager).ok();

        private_api::install_pairing_hooks();

        this
    }

    fn as_delegate(&self) -> &AnyObject {
        self.as_ref()
    }

    // RefCell borrows are never held across calls into IOBluetooth, since those may call back into the delegate.

    fn device_inquiry(&self) -> Option<Retained<IOBluetoothDeviceInquiry>> {
        self.ivars().device_inquiry.borrow().clone()
    }

    fn pairing_button(&self) -> PairingButton {
        PairingButton::for_attempt(self.ivars().failed_attempts.get())
    }

    fn is_busy(&self) -> bool {
        self.ivars().device_pair.borrow().is_some() || self.ivars().showing_result.get()
    }

    fn take_device_pair(&self) -> Option<Retained<IOBluetoothDevicePair>> {
        let pair = self.ivars().device_pair.borrow_mut().take();
        if pair.is_some() {
            private_api::clear_pin_reply();
        }
        pair
    }

    fn handle_state(&self, state: CBManagerState) {
        match state {
            CBManagerState::Unauthorized => self.show_fatal_error_alert(
                "Bluetooth Permission Denied",
                "WiimotePair is not allowed to access Bluetooth. Please allow WiimotePair to access Bluetooth in the \
                 \"Privacy & Security\" pane within the System Settings app.",
            ),
            CBManagerState::PoweredOff => {
                let inquiry = self.ivars().device_inquiry.borrow_mut().take();
                if let Some(inquiry) = inquiry {
                    unsafe { inquiry.stop() };
                }
                if let Some(pair) = self.take_device_pair() {
                    unsafe { pair.stop() };
                }

                self.show_fatal_error_alert(
                    "Bluetooth Unavailable",
                    "Please turn Bluetooth on before running WiimotePair.",
                );
            }
            CBManagerState::Unsupported | CBManagerState::Unknown => self.show_fatal_error_alert(
                "Unknown Bluetooth Error",
                "CBCentralManager is in an invalid state. Relaunch WiimotePair and try again.",
            ),
            CBManagerState::PoweredOn if self.device_inquiry().is_none() => {
                // SAFETY: The delegate is kept alive by AppDelegate for the lifetime of the app.
                let inquiry = unsafe { IOBluetoothDeviceInquiry::inquiryWithDelegate(Some(self.as_delegate())) };
                if let Some(inquiry) = inquiry {
                    unsafe { inquiry.setSearchType(IOBluetoothDeviceSearchTypesBits::Classic.0) };
                    *self.ivars().device_inquiry.borrow_mut() = Some(inquiry.clone());
                    let result = unsafe { inquiry.start() };
                    log!("Started inquiry: {}", describe_io_return(result));
                } else {
                    log!("Could not create a device inquiry");
                }
            }
            // TODO: handle CBManagerStateResetting?
            _ => {}
        }
    }

    fn handle_device_found(&self, inquiry: &IOBluetoothDeviceInquiry, device: &IOBluetoothDevice) {
        let name = unsafe { device.name() }.to_string();
        let address = unsafe { device.addressString() }.map(|address| address.to_string()).unwrap_or_default();

        // Skip unsupported devices.
        if !name.contains("Nintendo RVL-CNT-01") {
            // Clear devices to enable rediscovery if the name changes.
            unsafe { inquiry.clearFoundDevices() };
            return;
        }

        if self.is_busy() {
            return;
        }

        // Skip already paired devices.
        if unsafe { device.isPaired() } {
            log!("Found {name} ({address}), skipping since it is already paired");
            return;
        }

        log!("Found {name} ({address}), connected {}, starting pairing", unsafe { device.isConnected() });

        unsafe { inquiry.stop() };

        let Some(pair) = (unsafe { IOBluetoothDevicePair::pairWithDevice(Some(device)) }) else {
            self.show_pairing_result_alert("Pairing Error", "Could not create a pairing session for the Wii Remote.");
            return;
        };
        // SAFETY: The delegate is kept alive by AppDelegate for the lifetime of the app.
        unsafe { pair.setDelegate(Some(self.as_delegate())) };

        // We need to call this private API to ensure that the delegate is always queried for the PIN.
        if let Err(error) = private_api::set_user_defined_pincode(&pair, true) {
            self.show_pairing_result_alert("Pairing Error", &error.to_string());
            return;
        }

        // The pairing hooks reply to the Wii Remote's PIN code request while the pair is active.
        let prepared = pin_key_for(self.pairing_button(), device)
            .and_then(|key| private_api::prepare_pin_reply(device, key).map_err(|error| error.to_string()));
        if let Err(message) = prepared {
            self.show_pairing_result_alert("Pairing Error", &message);
            return;
        }

        *self.ivars().device_pair.borrow_mut() = Some(pair.clone());

        let result = unsafe { pair.start() };
        log!("Started pairing: {}", describe_io_return(result));
        // Whoever takes the pair out of device_pair reports the outcome, so each attempt shows exactly one alert even
        // if IOBluetooth calls devicePairingFinished:error: before returning.
        if result != K_IO_RETURN_SUCCESS && self.take_device_pair().is_some() {
            self.show_pairing_result_alert(
                "Pairing Error",
                &format!("An error occurred while starting the pairing process: {}.", describe_io_return(result)),
            );
        }
    }

    fn handle_pin_code_request(&self, pair: &IOBluetoothDevicePair) {
        log!("PIN code requested");

        let Some(device) = (unsafe { pair.device() }) else {
            log!("Pair has no device");
            return;
        };

        let result = pin_key_for(self.pairing_button(), &device)
            .and_then(|key| private_api::reply_pin_code(pair, &device, key).map_err(|error| error.to_string()));

        if let Err(message) = &result {
            log!("Could not reply with PIN code: {message}");
        }

        if let Err(message) = result
            && self.take_device_pair().is_some()
        {
            unsafe { pair.stop() };
            self.show_pairing_result_alert("Pairing Error", &message);
        }
    }

    fn handle_pairing_finished(&self, error: c_int) {
        log!("Pairing finished: {}", describe_pairing_error(error));

        // If there is no pair, the attempt was already abandoned and reported.
        let Some(pair) = self.take_device_pair() else {
            return;
        };
        unsafe { pair.stop() };

        if error != K_IO_RETURN_SUCCESS && self.ivars().failed_attempts.get() < AUTOMATIC_RETRIES {
            let failed_attempts = self.ivars().failed_attempts.get() + 1;
            self.ivars().failed_attempts.set(failed_attempts);
            log!("Retrying automatically ({failed_attempts} of {AUTOMATIC_RETRIES})");
            self.resume_inquiry();
        } else if error != K_IO_RETURN_SUCCESS {
            self.show_pairing_result_alert(
                "Pairing Error",
                &format!(
                    "An error occurred while attempting to pair: {}.\n\nPress the SYNC button on the back of the Wii \
                     Remote, then try again.",
                    describe_pairing_error(error)
                ),
            );
        } else {
            self.show_pairing_result_alert("Paired", "The Wii Remote has been paired with your Mac.");
        }
    }

    fn show_pairing_result_alert(&self, title: &str, text: &str) {
        self.ivars().failed_attempts.set(0);
        self.ivars().showing_result.set(true);
        let this = self.retain();
        self.show_alert(title, text, move || {
            // Resume searching so another Wii Remote (or another attempt with the same one) can be paired.
            this.ivars().showing_result.set(false);
            this.resume_inquiry();
        });
    }

    fn resume_inquiry(&self) {
        if let Some(inquiry) = self.device_inquiry() {
            unsafe {
                inquiry.clearFoundDevices();
                inquiry.start();
            }
        }
    }

    fn show_fatal_error_alert(&self, title: &str, text: &str) {
        let mtm = self.mtm();
        self.show_alert(title, text, move || NSApplication::sharedApplication(mtm).terminate(None));
    }

    fn show_alert(&self, title: &str, text: &str, callback: impl Fn() + 'static) {
        show_alert(&self.ivars().window, title, text, callback);
    }
}

/// The button that was pressed to put the Wii Remote into pairing mode, which decides the PIN it expects.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PairingButton {
    Sync,
    OneAndTwo,
}

impl PairingButton {
    /// There is no way to tell which button was pressed, so attempts alternate between them, starting with SYNC.
    fn for_attempt(attempt: u32) -> Self {
        if attempt.is_multiple_of(2) { Self::Sync } else { Self::OneAndTwo }
    }
}

/// Returns the PIN the Wii Remote expects, as a key for the pairing coordinator.
///
/// With the SYNC button, the PIN is the address of this Mac's Bluetooth controller in reverse. With the 1 and 2 buttons,
/// it is the Wii Remote's own address in reverse.
fn pin_key_for(button: PairingButton, device: &IOBluetoothDevice) -> Result<u64, String> {
    let (address_string, owner) = match button {
        PairingButton::Sync => (
            unsafe { IOBluetoothHostController::defaultController() }
                .and_then(|controller| unsafe { controller.addressAsString() }),
            "this Mac's Bluetooth controller",
        ),
        PairingButton::OneAndTwo => (unsafe { device.addressString() }, "the Wii Remote"),
    };
    let address_string = address_string.ok_or_else(|| format!("Could not determine the address of {owner}."))?;

    let mut address = BluetoothDeviceAddress { data: [0; 6] };
    // SAFETY: address is a valid pointer to a BluetoothDeviceAddress.
    let result = unsafe { IOBluetoothNSStringToDeviceAddress(Some(&address_string), &mut address) };
    if result != K_IO_RETURN_SUCCESS {
        return Err(format!("Could not parse the address of {owner}: {}.", describe_io_return(result)));
    }

    let key = pin_key(&address);
    log!(
        "Using the PIN for the {button:?} button, from the address of {owner} ({address_string}): {:02X?}",
        &key.to_ne_bytes()[..6]
    );
    Ok(key)
}

fn pin_key(address: &BluetoothDeviceAddress) -> u64 {
    let mut pin = [0u8; 8];
    for (pin_byte, address_byte) in pin.iter_mut().zip(address.data.iter().rev()) {
        *pin_byte = *address_byte;
    }
    u64::from_ne_bytes(pin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempts_alternate_between_buttons() {
        let buttons: Vec<_> = (0..4).map(PairingButton::for_attempt).collect();
        assert_eq!(
            buttons,
            [PairingButton::Sync, PairingButton::OneAndTwo, PairingButton::Sync, PairingButton::OneAndTwo]
        );
    }

    #[test]
    fn pin_key_is_reversed_address() {
        let address = BluetoothDeviceAddress { data: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55] };
        assert_eq!(pin_key(&address).to_ne_bytes(), [0x55, 0x44, 0x33, 0x22, 0x11, 0x00, 0x00, 0x00]);
    }
}
