// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

//! Diagnostic logging.
//!
//! Messages are written on a background thread to stderr and to `~/Library/Logs/WiimotePair.log`. When WiimotePair is
//! sandboxed, `~` is its container, so the file is at
//! `~/Library/Containers/org.dolphin-emu.WiimotePair/Data/Library/Logs/WiimotePair.log`. NSLog is not used, since the
//! unified log redacts its messages as `<private>`.

use std::ffi::{c_char, c_int, c_long};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

unsafe extern "C" {
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

struct Entry {
    time: SystemTime,
    main_thread: bool,
    message: String,
}

/// Messages are written on a separate thread, so that logging does not delay replies to pairing requests.
static LOG_SENDER: OnceLock<Mutex<Sender<Entry>>> = OnceLock::new();

/// Returns the path of the log file.
pub fn path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Logs/WiimotePair.log"))
}

fn open() -> Option<File> {
    let path = path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    File::create(path).ok()
}

/// Formats a time in local time like the unified log does, so that both logs can be compared.
fn timestamp(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs() as c_long;
    // SAFETY: An all-zero Tm is valid, and localtime_r only writes to it.
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    if unsafe { localtime_r(&seconds, &mut tm) }.is_null() {
        return format!("{}.{:06}", since_epoch.as_secs(), since_epoch.subsec_micros());
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        since_epoch.subsec_micros()
    )
}

fn start_writer() -> Mutex<Sender<Entry>> {
    let (sender, receiver) = channel::<Entry>();
    std::thread::spawn(move || {
        let mut file = open();
        for entry in receiver {
            let thread = if entry.main_thread { "main" } else { "background" };
            let line = format!("{} [{thread}] {}", timestamp(entry.time), entry.message);
            if let Some(file) = &mut file {
                let _ = writeln!(file, "{line}");
            }
            eprintln!("{line}");
        }
    });
    Mutex::new(sender)
}

pub fn write(message: String) {
    let entry = Entry { time: SystemTime::now(), main_thread: objc2::MainThreadMarker::new().is_some(), message };
    let sender = LOG_SENDER.get_or_init(start_writer).lock().unwrap_or_else(|error| error.into_inner());
    let _ = sender.send(entry);
}

macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(format!($($arg)*))
    };
}

pub(crate) use log;
