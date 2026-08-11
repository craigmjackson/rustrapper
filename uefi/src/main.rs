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

#[cfg(not(test))]
fn get_key() -> Option<u8> {
    unsafe {
        let st = SYSTEM_TABLE?;
        let con_in = &*(st.con_in as *mut EFI_SIMPLE_TEXT_INPUT_PROTOCOL);
        let mut key = EFI_INPUT_KEY { scan_code: 0, unicode_char: 0 };
        let status = (con_in.read_key_stroke)(con_in as *const _ as *mut _, &mut key);
        if status == EFI_SUCCESS {
            Some(key.unicode_char as u8)
        } else {
            None
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
                net::w16(con_out, "\nLua Shell (type 'exit' to return)\n\n");
                let mut state = lua::LuaState::new();
                state.register_builtins(u16_putc);
                state.set_fetch(None);
                let mut buf = [0u16; 256];
                let mut len = 0u32;
                loop {
                    let c = get_key();
                    match c {
                        Some(b'\r') | Some(b'\n') => {
                            if len > 0 {
                                net::w16(con_out, "\n");
                                let line = unsafe {
                                    core::str::from_utf8_unchecked(
                                        &core::slice::from_raw_parts(
                                            buf.as_ptr() as *const u8,
                                            len as usize,
                                        )
                                    )
                                };
                                match lua::eval::run_repl_once(&mut state, line.as_bytes(), u16_putc) {
                                    Ok(lua::eval::ExecResult::Normal) => {}
                                    Ok(lua::eval::ExecResult::Exit) => break,
                                    Ok(lua::eval::ExecResult::Shell) => {
                                        net::w16(con_out, "\n(nested shell not supported)\n\n");
                                    }
                                    Ok(lua::eval::ExecResult::Ret(_)) => {}
                                    Err(e) => {
                                        net::w16(con_out, "Lua error: ");
                                        net::w16(con_out, e);
                                        net::w16(con_out, "\n");
                                    }
                                }
                                len = 0;
                            }
                        }
                        Some(b'\x7f') | Some(b'\x08') => {
                            if len > 0 {
                                len -= 1;
                                u16_putc(b'\x08');
                                u16_putc(b' ');
                                u16_putc(b'\x08');
                            }
                        }
                        Some(ch) if ch >= 0x20 && ch < 0x7f && len < buf.len() as u32 - 1 => {
                            buf[len as usize] = ch as u16;
                            len += 1;
                            u16_putc(ch);
                        }
                        _ => {}
                    }
                }
                net::w16(con_out, "\n");
            }
        }
    }
}
