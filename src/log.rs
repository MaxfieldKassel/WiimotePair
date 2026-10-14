// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

//! Diagnostic logging through NSLog, which writes to stderr and to the unified log. Run
//! `log stream --predicate 'process == "WiimotePair"'` to follow it when WiimotePair is launched from Finder.

use objc2_foundation::{NSString, ns_string};

unsafe extern "C" {
    fn NSLog(format: *const NSString, ...);
}

pub fn write(message: &str) {
    let message = NSString::from_str(message);
    // SAFETY: The format string consumes exactly one object argument.
    unsafe { NSLog(ns_string!("%@"), &*message as *const NSString) };
}

macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(&format!($($arg)*))
    };
}

pub(crate) use log;
