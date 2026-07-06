#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

mod efi;
#[cfg(not(test))]
mod scan;

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
use crate::efi::*;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[cfg(not(test))]
#[export_name = "efi_main"]
pub extern "efiapi" fn efi_main(image_handle: EFI_HANDLE, system_table: &EFI_SYSTEM_TABLE) -> EFI_STATUS {
    scan::scan_storage_devices(image_handle, system_table)
}
