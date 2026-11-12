//! Watches the clipboard for `/showlocation` lines. Reads only when the sequence
//! number changes, and drops anything that isn't exactly that shape unseen.

use std::ops::ControlFlow;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use sc_nav_geo::V3;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

const CF_UNICODETEXT: u32 = 13;
/// Longer than any `/showlocation` line, so longer text is never copied out.
const MAX_CHARS: usize = 128;

/// Calls `on_fix(detected_at, unix_ms, xyz)` for each new `/showlocation`
/// on the clipboard, until it breaks.
pub fn watch(mut on_fix: impl FnMut(Instant, i64, V3) -> ControlFlow<()>) {
    let mut seq = unsafe { GetClipboardSequenceNumber() };
    let mut last = None;
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let now = unsafe { GetClipboardSequenceNumber() };
        if now == seq {
            continue;
        }
        seq = now;
        // Stamp at detection, before the read's own retries.
        let at = Instant::now();
        // the corrected clock: a /showlocation fix is converted with it
        let unix_ms = sc_nav_clock::now_ms();
        let Some(xyz) = read_text()
            .as_deref()
            .and_then(sc_nav_geo::parse_showlocation)
        else {
            continue;
        };
        if last == Some(xyz) {
            continue;
        }
        last = Some(xyz);
        if on_fix(at, unix_ms, xyz).is_break() {
            return;
        }
    }
}

fn read_text() -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return None;
        }
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(null_mut()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if !opened {
            return None;
        }
        let text = copy_text();
        CloseClipboard();
        text
    }
}

unsafe fn copy_text() -> Option<String> {
    unsafe {
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            return None;
        }
        let units = GlobalSize(handle) / 2;
        if units == 0 || units > MAX_CHARS + 1 {
            return None;
        }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let wide = std::slice::from_raw_parts(ptr, units);
        let len = wide.iter().position(|&c| c == 0).unwrap_or(units);
        let text = String::from_utf16(&wide[..len]).ok();
        GlobalUnlock(handle);
        text
    }
}
