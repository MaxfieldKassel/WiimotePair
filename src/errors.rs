// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

use std::ffi::{CStr, c_char, c_int};

use objc2_io_bluetooth::{
    kBluetoothHCIErrorAuthenticationFailure, kBluetoothHCIErrorConnectionTerminatedByLocalHost,
    kBluetoothHCIErrorConnectionTimeout, kBluetoothHCIErrorHostRejectedSecurityReasons, kBluetoothHCIErrorHostTimeout,
    kBluetoothHCIErrorInsufficientSecurity, kBluetoothHCIErrorKeyMissing, kBluetoothHCIErrorLMPResponseTimeout,
    kBluetoothHCIErrorNoConnection, kBluetoothHCIErrorOtherEndTerminatedConnectionUserEnded,
    kBluetoothHCIErrorPageTimeout, kBluetoothHCIErrorPairingNotAllowed,
    kBluetoothHCIErrorPairingWithUnitKeyNotSupported, kBluetoothHCIErrorRepeatedAttempts,
    kBluetoothHCIErrorUnspecifiedError,
};

unsafe extern "C" {
    fn mach_error_string(error_value: c_int) -> *const c_char;
}

/// Describes an IOReturn value, such as one returned by `-[IOBluetoothDevicePair start]`.
pub fn describe_io_return(error: c_int) -> String {
    // SAFETY: mach_error_string accepts any value and returns a static, NUL-terminated string.
    let description = unsafe { CStr::from_ptr(mach_error_string(error)) };
    format!("\"{}\"", description.to_string_lossy())
}

/// Describes an error passed to `-[IOBluetoothDevicePairDelegate devicePairingFinished:error:]`.
///
/// In practice, IOBluetoothDevicePair reports pairing failures as Bluetooth HCI status codes rather than IOReturn
/// values. Passing these to mach_error_string() produces unrelated messages, e.g. HCI 0x1F (Unspecified Error) becomes
/// "(os/kern) default set" and HCI 0x02 (No Connection) becomes "(os/kern) protection failure".
#[allow(non_upper_case_globals)]
pub fn describe_pairing_error(error: c_int) -> String {
    if !(1..=0xFF).contains(&error) {
        return describe_io_return(error);
    }

    let description = match error as u32 {
        kBluetoothHCIErrorNoConnection => "No connection",
        kBluetoothHCIErrorPageTimeout => "Page timeout",
        kBluetoothHCIErrorAuthenticationFailure => "Authentication failure",
        kBluetoothHCIErrorKeyMissing => "PIN or key missing",
        kBluetoothHCIErrorConnectionTimeout => "Connection timeout",
        kBluetoothHCIErrorHostRejectedSecurityReasons => "Connection rejected due to security reasons",
        kBluetoothHCIErrorHostTimeout => "Connection accept timeout",
        kBluetoothHCIErrorOtherEndTerminatedConnectionUserEnded => "Remote device terminated the connection",
        kBluetoothHCIErrorConnectionTerminatedByLocalHost => "Connection terminated by local host",
        kBluetoothHCIErrorRepeatedAttempts => "Repeated attempts",
        kBluetoothHCIErrorPairingNotAllowed => "Pairing not allowed",
        kBluetoothHCIErrorUnspecifiedError => "Unspecified error",
        kBluetoothHCIErrorLMPResponseTimeout => "LMP response timeout",
        kBluetoothHCIErrorPairingWithUnitKeyNotSupported => "Pairing with unit key not supported",
        kBluetoothHCIErrorInsufficientSecurity => "Insufficient security",
        _ => "Unknown error",
    };

    format!("\"{description}\" (Bluetooth error 0x{error:02X})")
}
