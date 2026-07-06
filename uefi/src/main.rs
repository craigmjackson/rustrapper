#![no_std]
#![no_main]

mod efi;
mod scan;

use core::panic::PanicInfo;

use crate::efi::*;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[export_name = "efi_main"]
pub extern "efiapi" fn efi_main(image_handle: EFI_HANDLE, system_table: &EFI_SYSTEM_TABLE) -> EFI_STATUS {
    scan::scan_storage_devices(image_handle, system_table)
}
