// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

//! Diagnostic logging.
//!
//! Messages go to NSLog, which writes to stderr and to the unified log. However, the unified log redacts their
//! contents as `<private>`, so they are also written to `~/Library/Logs/WiimotePair.log`. When WiimotePair is
//! sandboxed, `~` is its container, so the file is at
//! `~/Library/Containers/org.dolphin-emu.WiimotePair/Data/Library/Logs/WiimotePair.log`.

use std::ffi::{c_char, c_int, c_long};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use objc2_foundation::{NSString, ns_string};

unsafe extern "C" {
    fn NSLog(format: *const NSString, ...);
    fn localtime_r(time: *const c_long, result: *mut Tm) -> *mut Tm;
}

#[repr(C)]
struct Tm {
    tm_sec: c_int,
    tm_min: c_int,
    tm_hour: c_int,
    tm_mday: c_int,
    tm_mon: c_int,
    tm_year: c_int,
    tm_wday: c_int,
    tm_yday: c_int,
    tm_isdst: c_int,
    tm_gmtoff: c_long,
    tm_zone: *const c_char,
}

static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

/// Returns the path of the log file.
pub fn path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Logs/WiimotePair.log"))
}

fn open() -> Option<Mutex<File>> {
    let path = path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    File::create(path).ok().map(Mutex::new)
}

/// Formats the current local time like the unified log does, so that both logs can be compared.
fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = now.as_secs() as c_long;
    // SAFETY: An all-zero Tm is valid, and localtime_r only writes to it.
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    if unsafe { localtime_r(&seconds, &mut tm) }.is_null() {
        return format!("{}.{:03}", now.as_secs(), now.subsec_millis());
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        now.subsec_millis()
    )
}

pub fn write(message: &str) {
    if let Some(file) = LOG_FILE.get_or_init(open) {
        let mut file = file.lock().unwrap_or_else(|error| error.into_inner());
        let thread = if objc2::MainThreadMarker::new().is_some() { "main" } else { "background" };
        let _ = writeln!(file, "{} [{thread}] {message}", timestamp());
    }

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
