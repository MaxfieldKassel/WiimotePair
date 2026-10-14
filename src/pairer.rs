// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

use std::cell::{OnceCell, RefCell};
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

pub struct PairerIvars {
    window: Retained<NSWindow>,
    central_manager: OnceCell<Retained<CBCentralManager>>,
    device_inquiry: RefCell<Option<Retained<IOBluetoothDeviceInquiry>>>,
    device_pair: RefCell<Option<Retained<IOBluetoothDevicePair>>>,
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
            // Restart inquiries that have timed out.
            if let (Some(sender), false) = (sender, aborted) {
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

        private_api::install_pairing_hook();

        this
    }

    fn as_delegate(&self) -> &AnyObject {
        self.as_ref()
    }

    // RefCell borrows are never held across calls into IOBluetooth, since those may call back into the delegate.

    fn device_inquiry(&self) -> Option<Retained<IOBluetoothDeviceInquiry>> {
        self.ivars().device_inquiry.borrow().clone()
    }

    fn take_device_pair(&self) -> Option<Retained<IOBluetoothDevicePair>> {
        let pair = self.ivars().device_pair.borrow_mut().take();
        if pair.is_some() {
            private_api::set_pairing_key(None);
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

        // The pairing hook replies to the Wii Remote's PIN code request with this key while the pair is active.
        let key = match controller_pin_key() {
            Ok(key) => key,
            Err(message) => {
                self.show_pairing_result_alert("Pairing Error", &message);
                return;
            }
        };
        private_api::set_pairing_key(Some(key));

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

        let result = controller_pin_key()
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

        if error != K_IO_RETURN_SUCCESS {
            self.show_pairing_result_alert(
                "Pairing Error",
                &format!(
                    "An error occurred while attempting to pair: {}.\n\nMake sure to press the SYNC button on the back \
                     of the Wii Remote, not the 1 and 2 buttons.",
                    describe_pairing_error(error)
                ),
            );
        } else {
            self.show_pairing_result_alert("Paired", "The Wii Remote has been paired with your Mac.");
        }
    }

    fn show_pairing_result_alert(&self, title: &str, text: &str) {
        let this = self.retain();
        self.show_alert(title, text, move || {
            // Resume searching so another Wii Remote (or another attempt with the same one) can be paired.
            if let Some(inquiry) = this.device_inquiry() {
                unsafe {
                    inquiry.clearFoundDevices();
                    inquiry.start();
                }
            }
        });
    }

    fn show_fatal_error_alert(&self, title: &str, text: &str) {
        let mtm = self.mtm();
        self.show_alert(title, text, move || NSApplication::sharedApplication(mtm).terminate(None));
    }

    fn show_alert(&self, title: &str, text: &str, callback: impl Fn() + 'static) {
        show_alert(&self.ivars().window, title, text, callback);
    }
}

/// Returns the PIN the Wii Remote expects when pairing using the SYNC button, as a key for the pairing coordinator.
///
/// When using the SYNC button, the PIN is the address of the Bluetooth controller in reverse.
fn controller_pin_key() -> Result<u64, String> {
    let address_string = unsafe { IOBluetoothHostController::defaultController() }
        .and_then(|controller| unsafe { controller.addressAsString() })
        .ok_or_else(|| "Could not determine the address of this Mac's Bluetooth controller.".to_string())?;
    log!("Bluetooth controller address: {address_string}");

    let mut address = BluetoothDeviceAddress { data: [0; 6] };
    // SAFETY: address is a valid pointer to a BluetoothDeviceAddress.
    let result = unsafe { IOBluetoothNSStringToDeviceAddress(Some(&address_string), &mut address) };
    if result != K_IO_RETURN_SUCCESS {
        return Err(format!(
            "Could not parse the address of this Mac's Bluetooth controller: {}.",
            describe_io_return(result)
        ));
    }

    let key = pin_key(&address);
    log!("PIN bytes: {:02X?}", &key.to_ne_bytes()[..6]);
    Ok(key)
}

fn pin_key(controller_address: &BluetoothDeviceAddress) -> u64 {
    let mut pin = [0u8; 8];
    for (pin_byte, address_byte) in pin.iter_mut().zip(controller_address.data.iter().rev()) {
        *pin_byte = *address_byte;
    }
    u64::from_ne_bytes(pin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_key_is_reversed_controller_address() {
        let address = BluetoothDeviceAddress { data: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55] };
        assert_eq!(pin_key(&address).to_ne_bytes(), [0x55, 0x44, 0x33, 0x22, 0x11, 0x00, 0x00, 0x00]);
    }
}
