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
use core::panic::PanicInfo;

#[cfg(not(test))]
use common::menu::{show_menu, MenuAction};

#[cfg(not(test))]
use crate::efi::*;

#[cfg(not(test))]
static mut SYSTEM_TABLE: Option<&'static EFI_SYSTEM_TABLE> = None;

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
fn u16_putc(c: u8) {
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

    match show_menu(u16_puts, u16_putc, get_key) {
        MenuAction::StorageScan => scan::scan_storage_devices(image_handle, system_table),
        MenuAction::NetworkBoot => {
            net::scan_network_devices(image_handle, system_table);
            loop {}
        }
    }
}
