#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

mod efi;
#[cfg(not(test))]
mod scan;
#[cfg(not(test))]
mod net;
#[cfg(not(test))]
mod mem;
#[cfg(not(test))]
mod loader;
#[cfg(not(test))]
mod fetch;

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
use common::menu::{show_menu, MenuAction};

#[cfg(not(test))]
use crate::efi::*;

#[cfg(not(test))]
pub static mut SYSTEM_TABLE: Option<&'static EFI_SYSTEM_TABLE> = None;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[cfg(not(test))]
fn u16_puts(s: &str) {
    if let Some(st) = unsafe { SYSTEM_TABLE } {
        let con_out = unsafe { &*st.con_out };
        net::w16(con_out, s);
    }
}

#[cfg(not(test))]
pub fn u16_putc(c: u8) {
    let mut buf = [0u16; 2];
    buf[0] = c as u16;
    if let Some(st) = unsafe { SYSTEM_TABLE } {
        let con_out = unsafe { &*st.con_out };
        unsafe {
            (con_out.output_string)(con_out as *const _ as *mut _, buf.as_ptr());
        }
    }
}

// State for multi-byte escape sequences (ESC [ 3 ~ etc.) that arrive across
// several ReadKeyStroke polls (serial-terminal input under -nographic).
#[cfg(not(test))]
const ESC_BUF_LEN: usize = 8;
#[cfg(not(test))]
static mut ESC_SEQ: [u8; ESC_BUF_LEN] = [0; ESC_BUF_LEN];
#[cfg(not(test))]
static mut ESC_N: usize = 0;

// Temporary diagnostic: report raw scan/unicode for unrecognized keys so the
// ARM64 UEFI backspace/delete key delivery can be identified precisely.
#[cfg(not(test))]
fn debug_key(scan: u16, unicode: u16) {
    let hex = b"0123456789ABCDEF";
    let mut buf = [0u8; 24];
    let mut i = 0;
    for &(label, v) in &[(b's', scan), (b'u', unicode)] {
        buf[i] = label;
        i += 1;
        buf[i] = b'=';
        i += 1;
        for shift in [12u32, 8, 4, 0] {
            buf[i] = hex[((v >> shift) & 0xF) as usize];
            i += 1;
        }
        buf[i] = b' ';
        i += 1;
    }
    buf[i] = b'\r';
    i += 1;
    buf[i] = b'\n';
    i += 1;
    let s = core::str::from_utf8(&buf[..i]).unwrap_or("");
    u16_puts(s);
}

#[cfg(not(test))]
fn get_key() -> Option<u8> {
    loop {
        unsafe {
            let st = SYSTEM_TABLE?;
            let con_in = &*(st.con_in as *mut EFI_SIMPLE_TEXT_INPUT_PROTOCOL);
            let mut key = EFI_INPUT_KEY { scan_code: 0, unicode_char: 0 };
            let status = (con_in.read_key_stroke)(con_in as *const _ as *mut _, &mut key);
            if status != EFI_SUCCESS {
                return None;
            }
            let scan = key.scan_code;
            let unicode = key.unicode_char;
            let ch = unicode as u8;

            // Continue a partially-received escape sequence across polls.
            if ESC_N > 0 {
                // ESC followed by anything but '['/'O' is a lone ESC, not a
                // sequence: cancel and reprocess this key normally.
                let cancel = ESC_N == 1 && ch != b'[' && ch != b'O';
                if !cancel {
                    if ESC_N < ESC_BUF_LEN {
                        ESC_SEQ[ESC_N] = ch;
                    }
                    ESC_N += 1;
                    // Sequences end at '~', 'M', or a letter.
                    if ch == b'~' || ch == b'M' || ch.is_ascii_alphabetic() {
                        // Only ESC [ 3 ~ (Delete) maps to backspace; arrows and
                        // other sequences are discarded.
                        let delete = ESC_N == 4
                            && ESC_SEQ[0] == b'\x1B'
                            && ESC_SEQ[1] == b'['
                            && ESC_SEQ[2] == b'3'
                            && ESC_SEQ[3] == b'~';
                        ESC_N = 0;
                        return if delete { Some(b'\x7F') } else { None };
                    }
                    return None;
                }
                ESC_N = 0;
            }

            // Backspace: unicode 0x08 / 0x7F (DEL), or EDK2 SCAN_DELETE (0x0008).
            if ch == b'\x08' || ch == b'\x7F' || scan == 0x0008 {
                return Some(b'\x7F');
            }
            // Escape: unicode 0x1B or EDK2 SCAN_ESC (0x0017) starts a sequence.
            if ch == b'\x1B' || scan == 0x0017 {
                ESC_SEQ[0] = b'\x1B';
                ESC_N = 1;
                return None;
            }
            // Enter / line feeds must reach the REPL's enter handling.
            if ch == b'\r' || ch == b'\n' {
                return Some(ch);
            }
            if unicode > 0 && ch >= 0x20 && ch < 0x7F {
                return Some(ch);
            }
            debug_key(scan, unicode);
            return None;
        }
    }
}

#[cfg(not(test))]
#[export_name = "efi_main"]
pub extern "efiapi" fn efi_main(image_handle: EFI_HANDLE, system_table: &'static EFI_SYSTEM_TABLE) -> ! {
    unsafe { SYSTEM_TABLE = Some(system_table); }
    let con_out = unsafe { &*system_table.con_out };
    net::w16(con_out, "Rustrapper UEFI\r\n");

    loop {
        match show_menu(u16_puts, u16_putc, get_key) {
            MenuAction::StorageScan => scan::scan_storage_devices(image_handle, system_table),
            MenuAction::NetworkBoot => net::scan_network_devices(image_handle, system_table),
            MenuAction::LuaShell => {
                let mut state = lua::LuaState::new();
                state.register_builtins(u16_putc);
                state.set_fetch(None);
                lua::repl::repl_loop(&mut state, get_key, u16_putc, u16_puts);
            }
        }
    }
}
